/* =============================================================================
 * File          : database.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SQLite database bootstrap: opens the single application database,
 *   applies the embedded SQL migrations (compiled into the binary), guards
 *   against running an older binary on a newer schema, checks the file's
 *   integrity, and exposes the shared connection pool every repository
 *   builds on.
 *
 * Dependencies:
 *   - sqlx (>= 0.9, sqlite), include_dir (>= 0.7)
 *
 * Usage:
 *   Initialized once at startup; Database is cloned into AppState.
 * =============================================================================
 */

//! SQLite database bootstrap (embedded migrations + shared pool).

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
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    /// The database was written by a newer HomeVault than this binary.
    ///
    /// Migrations are append-only and never reverted, so starting an older
    /// binary against a newer schema silently succeeds and then fails on the
    /// first query that expects a column it does not know about. Refusing to
    /// start is the only safe answer.
    #[error(
        "Installed database uses schema version {db_version}, but this HomeVault build only \
         understands up to {binary_version}. Reinstall the newer HomeVault version (or restore \
         a database snapshot from before the upgrade)."
    )]
    SchemaTooNew {
        db_version: i64,
        binary_version: i64,
    },
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

    // -----------------------------------------------------------------
    // Schema version guard
    // -----------------------------------------------------------------

    /// Highest migration number embedded in this binary.
    ///
    /// Derived from the file names (`047_user_lockout_expiry.sql` → 47) rather
    /// than a hand-maintained constant: a constant drifts the first time
    /// somebody adds a migration and forgets to bump it, which would silently
    /// disable the guard.
    pub fn current_schema_version() -> i64 {
        MIGRATIONS_DIR
            .files()
            .filter_map(|f| f.path().file_name().and_then(|n| n.to_str()))
            .filter_map(|name| name.split('_').next())
            .filter_map(|prefix| prefix.parse::<i64>().ok())
            .max()
            .unwrap_or(0)
    }

    /// Names of the embedded migrations that have not been applied yet.
    pub async fn pending_migrations(&self) -> Vec<String> {
        let applied: Vec<String> = sqlx::query_scalar("SELECT name FROM _hv_migrations")
            .fetch_all(self.pool.as_ref())
            .await
            .unwrap_or_default();
        let mut pending: Vec<String> = MIGRATIONS_DIR
            .files()
            .filter_map(|f| {
                f.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            })
            .filter(|name| !applied.contains(name))
            .collect();
        pending.sort();
        pending
    }

    /// Schema version recorded in the database, if any.
    ///
    /// `app_meta` only exists from migration 048 onwards, so an older install
    /// answers `None` and the guard treats that as "not newer than us".
    ///
    /// **An unreadable `app_meta` is treated as "unknown", not as an error.**
    /// A damaged page there would otherwise fail the guard, which runs before
    /// anything else and would refuse the whole startup — the opposite of the
    /// intended "degrade rather than refuse to boot" behaviour, and precisely
    /// when the operator needs the appliance up to restore a snapshot. The
    /// damage is caught a few lines later by `quick_check`, which surfaces it
    /// through `/api/health/ready`.
    async fn stored_schema_version(&self) -> Result<Option<i64>, DbError> {
        let table_exists: bool = match sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'app_meta'",
        )
        .fetch_one(self.pool.as_ref())
        .await
        {
            Ok(n) => n > 0,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Could not read the schema catalogue; treating the schema version as unknown"
                );
                return Ok(None);
            }
        };

        if !table_exists {
            return Ok(None);
        }

        match sqlx::query_scalar("SELECT schema_version FROM app_meta WHERE id = 1")
            .fetch_optional(self.pool.as_ref())
            .await
        {
            Ok(version) => Ok(version),
            Err(e) => {
                // Corruption, not a version mismatch. Let the integrity check
                // report it instead of blocking the boot.
                tracing::warn!(
                    error = %e,
                    "Could not read app_meta.schema_version; treating it as unknown"
                );
                Ok(None)
            }
        }
    }

    /// Record the schema version this binary just brought the database to.
    pub async fn record_schema_version(&self) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO app_meta (id, schema_version, app_version, updated_at) \
             VALUES (1, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
             ON CONFLICT(id) DO UPDATE SET \
               schema_version = excluded.schema_version, \
               app_version = excluded.app_version, \
               updated_at = excluded.updated_at",
        )
        .bind(Self::current_schema_version())
        .bind(env!("CARGO_PKG_VERSION"))
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Refuse to run an older binary against a newer database.
    async fn guard_schema_version(&self) -> Result<(), DbError> {
        let Some(stored) = self.stored_schema_version().await? else {
            return Ok(());
        };
        let binary = Self::current_schema_version();
        if stored > binary {
            return Err(DbError::SchemaTooNew {
                db_version: stored,
                binary_version: binary,
            });
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Snapshots & integrity
    // -----------------------------------------------------------------

    /// Write a self-contained snapshot of the database to `dest`.
    ///
    /// `VACUUM INTO` is the only correct way to copy a WAL database: it emits
    /// one file containing every committed page, so the copy needs neither the
    /// `-wal` nor the `-shm` sidecar. Plain `cp homevault.db` on a live WAL
    /// database yields a file with *no tables at all*.
    ///
    /// Callers write to a temporary name and rename, so an interrupted snapshot
    /// never leaves a half-written file that looks restorable.
    ///
    /// Caveat, measured: against an **in-memory** database this returns `Ok(())`
    /// and writes nothing at all. The appliance always runs on a file, and the
    /// tests that exercise snapshots therefore use a temp file rather than
    /// `sqlite::memory:`.
    pub async fn vacuum_into(&self, dest: &std::path::Path) -> Result<(), DbError> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `VACUUM INTO` takes an *expression*, so the destination binds as a
        // normal parameter — no string interpolation, hence no SQL injection
        // surface and no dynamic-SQL audit flag.
        let target = dest.to_string_lossy().to_string();
        sqlx::query("VACUUM INTO ?")
            .bind(&target)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Fold the write-ahead log back into the main database file.
    ///
    /// Runs periodically so the WAL cannot grow without bound on a busy
    /// appliance — an unbounded `-wal` is both a disk-space hazard and the
    /// reason a naive copy of the `.db` file looks empty.
    pub async fn wal_checkpoint(&self) -> Result<(), DbError> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Fast structural check (`PRAGMA quick_check`), suitable for every boot.
    ///
    /// Returns `Ok(None)` when the database is sound, or `Ok(Some(reason))`
    /// with SQLite's own message when it is not. A corrupt file must not stop
    /// the appliance from starting: a degraded server that can still export the
    /// data is far more useful than one that refuses to boot.
    pub async fn quick_check(&self) -> Result<Option<String>, DbError> {
        let rows: Vec<String> = sqlx::query_scalar("PRAGMA quick_check(1)")
            .fetch_all(self.pool.as_ref())
            .await?;
        if rows.len() == 1 && rows[0].eq_ignore_ascii_case("ok") {
            Ok(None)
        } else {
            Ok(Some(rows.join("; ")))
        }
    }

    /// Full integrity check (`PRAGMA integrity_check`) — slow, on demand only.
    pub async fn integrity_check(&self) -> Result<Option<String>, DbError> {
        let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(self.pool.as_ref())
            .await?;
        if rows.len() == 1 && rows[0].eq_ignore_ascii_case("ok") {
            Ok(None)
        } else {
            Ok(Some(rows.join("; ")))
        }
    }

    /// Run all pending `.sql` migrations (embedded in the binary at compile
    /// time from `sqlx/migrations/`) in sorted order.
    ///
    /// Refuses to touch a database written by a *newer* HomeVault (see
    /// [`DbError::SchemaTooNew`]): migrations are append-only, so an older
    /// binary would otherwise start happily and then fail on the first query
    /// expecting a column it does not know about.
    pub async fn run_migrations(&self) -> Result<(), DbError> {
        self.guard_schema_version().await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh in-memory database with every migration applied.
    async fn migrated_db() -> Database {
        let db = Database::new("sqlite::memory:")
            .await
            .expect("open in-memory database");
        db.run_migrations().await.expect("run migrations");
        db
    }

    /// A unique temp path so parallel tests never collide.
    fn temp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("hv-db-{tag}-{}-{nanos}.db", std::process::id()))
    }

    // =================================================================
    // Schema version
    // =================================================================

    /// The guard is derived from the migration file names, so it must track the
    /// highest one — this is the test that fails if somebody adds a migration
    /// and the guard silently stops covering it.
    #[tokio::test]
    async fn test_current_schema_version_is_the_highest_migration() {
        let version = Database::current_schema_version();
        assert!(version > 0, "the embedded migrations define a version");
        assert!(
            version >= 48,
            "the app_meta migration is 048, so the version must be at least 48 (got {version})"
        );
    }

    #[tokio::test]
    async fn test_record_and_read_back_the_schema_version() {
        let db = migrated_db().await;
        // Freshly migrated, nothing recorded yet: the migration only creates
        // the table.
        assert_eq!(db.stored_schema_version().await.expect("read"), None);

        db.record_schema_version().await.expect("record");
        assert_eq!(
            db.stored_schema_version().await.expect("read back"),
            Some(Database::current_schema_version())
        );
    }

    /// An older binary must refuse to run against a newer database rather than
    /// failing later on a query it cannot satisfy.
    #[tokio::test]
    async fn test_run_migrations_refuses_a_newer_database() {
        let db = migrated_db().await;
        db.record_schema_version().await.expect("record");
        sqlx::query("UPDATE app_meta SET schema_version = 999 WHERE id = 1")
            .execute(db.pool().as_ref())
            .await
            .expect("bump version");

        let err = db.run_migrations().await.expect_err("must refuse");
        match err {
            DbError::SchemaTooNew {
                db_version,
                binary_version,
            } => {
                assert_eq!(db_version, 999);
                assert_eq!(binary_version, Database::current_schema_version());
            }
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
    }

    /// Equal or older recorded versions are fine (an upgrade, or an install
    /// predating the `app_meta` table).
    #[tokio::test]
    async fn test_run_migrations_accepts_an_equal_or_older_version() {
        let db = migrated_db().await;
        db.record_schema_version().await.expect("record");
        db.run_migrations().await.expect("same version is fine");

        sqlx::query("UPDATE app_meta SET schema_version = 1 WHERE id = 1")
            .execute(db.pool().as_ref())
            .await
            .expect("lower version");
        db.run_migrations().await.expect("lower version is fine");
    }

    /// A **damaged** `app_meta` must not fail the guard.
    ///
    /// The guard runs before anything else, so propagating the error would
    /// refuse the whole startup — exactly when the operator needs the appliance
    /// up to restore a snapshot. The damage is reported by `quick_check`
    /// instead (and surfaced through `/api/health/ready`).
    #[tokio::test]
    async fn test_unreadable_app_meta_is_treated_as_unknown_not_fatal() {
        let db = migrated_db().await;
        // Replace the table with a view-less stub that answers with the wrong
        // shape, which makes the `schema_version` query fail the way corruption
        // does — without needing to corrupt the file.
        sqlx::query("DROP TABLE app_meta")
            .execute(db.pool().as_ref())
            .await
            .expect("drop");
        sqlx::query("CREATE TABLE app_meta (id INTEGER PRIMARY KEY, not_a_version TEXT)")
            .execute(db.pool().as_ref())
            .await
            .expect("create broken table");

        assert_eq!(
            db.stored_schema_version().await.expect("must not error"),
            None,
            "an unreadable app_meta is 'unknown', not a fatal error"
        );
        assert!(
            db.guard_schema_version().await.is_ok(),
            "the guard must let the appliance start so a snapshot can be restored"
        );
    }

    /// A database that predates `app_meta` answers `None` — not an error.
    #[tokio::test]
    async fn test_missing_app_meta_is_not_a_schema_error() {
        let db = Database::new("sqlite::memory:").await.expect("open");
        assert_eq!(db.stored_schema_version().await.expect("read"), None);
        assert!(db.guard_schema_version().await.is_ok());
    }

    // =================================================================
    // Snapshots
    // =================================================================

    /// The whole point of `VACUUM INTO`: the copy is self-contained. A plain
    /// file copy of a WAL database can contain no tables at all.
    #[tokio::test]
    async fn test_vacuum_into_writes_a_complete_standalone_copy() {
        let source_path = temp_path("vacuum-source");
        let source_url = format!("sqlite:{}?mode=rwc", source_path.display());
        let db = Database::new(&source_url).await.expect("open file db");
        db.run_migrations().await.expect("migrate");
        db.record_schema_version().await.expect("record");
        sqlx::query(
            "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed) \
             VALUES ('snapshotuser', 'x', '', '', 1, 0, '2026-01-01T00:00:00Z', 's')",
        )
        .execute(db.pool().as_ref())
        .await
        .expect("insert user");

        let dest = temp_path("vacuum-dest");
        db.vacuum_into(&dest).await.expect("vacuum into");
        assert!(dest.exists(), "the snapshot file must exist");

        // Open the copy on its own — no -wal, no -shm, no source connection.
        // Deliberately not `Database::new`: its `configure_sqlite` sets WAL,
        // which needs a write and would fail on a read-only open.
        let copy_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&dest)
                    .read_only(true),
            )
            .await
            .expect("open the snapshot standalone");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'snapshotuser'")
                .fetch_one(&copy_pool)
                .await
                .expect("the snapshot must contain the table and its row");
        assert_eq!(count, 1);
        // ... and it really is self-contained: the sidecar files are not needed.
        assert!(!std::path::Path::new(&format!("{}-wal", dest.display())).exists());
        copy_pool.close().await;

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&dest);
    }

    /// Documents what SQLite actually does with an existing destination.
    ///
    /// SQLite either refuses (`output file already exists`) or replaces it,
    /// depending on the version. `DbBackupService` never relies on either
    /// behaviour: it writes to a `.tmp` name and renames, which is atomic and
    /// leaves nothing half-written. This test pins the fact that pointing
    /// `vacuum_into` at an existing path is *not* a supported contract.
    #[tokio::test]
    async fn test_vacuum_into_does_not_promise_overwrite_semantics() {
        let db = migrated_db().await;
        let dest = temp_path("vacuum-existing");
        std::fs::write(&dest, b"already here").expect("seed file");

        let result = db.vacuum_into(&dest).await;
        // Whatever happens, the caller must not end up with a *corrupt* file:
        // either the call failed and the old bytes are intact, or it produced a
        // valid SQLite database.
        match result {
            Err(_) => {
                assert_eq!(std::fs::read(&dest).expect("still there"), b"already here");
            }
            Ok(()) => {
                let copy_pool = SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect_with(
                        sqlx::sqlite::SqliteConnectOptions::new()
                            .filename(&dest)
                            .read_only(true),
                    )
                    .await
                    .expect("an overwritten destination must be a valid database");
                copy_pool.close().await;
            }
        }

        let _ = std::fs::remove_file(&dest);
    }

    // =================================================================
    // Integrity
    // =================================================================

    #[tokio::test]
    async fn test_quick_check_passes_on_a_healthy_database() {
        let db = migrated_db().await;
        assert_eq!(db.quick_check().await.expect("quick check"), None);
        assert_eq!(db.integrity_check().await.expect("integrity check"), None);
    }

    #[tokio::test]
    async fn test_quick_check_detects_a_corrupted_file() {
        let path = temp_path("corrupt");
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let db = Database::new(&url).await.expect("open");
        db.run_migrations().await.expect("migrate");
        // Force the pages out of the WAL so the file on disk is the real thing.
        db.wal_checkpoint().await.expect("checkpoint");
        drop(db);

        // Corrupt the middle of the file, past the header.
        let mut bytes = std::fs::read(&path).expect("read db");
        assert!(bytes.len() > 8192, "the database should span several pages");
        for byte in bytes.iter_mut().skip(4096).take(4096) {
            *byte = 0xFF;
        }
        std::fs::write(&path, &bytes).expect("write corrupted db");

        let db = Database::new(&url).await.expect("reopen");
        // Either the check reports damage, or SQLite refuses the page outright;
        // both mean "not ok", which is what the readiness probe needs.
        let damaged = matches!(db.quick_check().await, Ok(Some(_)) | Err(_));
        assert!(damaged, "a corrupted database must not report healthy");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_wal_checkpoint_truncates_the_log() {
        let path = temp_path("wal");
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let db = Database::new(&url).await.expect("open");
        db.run_migrations().await.expect("migrate");

        db.wal_checkpoint().await.expect("checkpoint");
        let wal = std::path::PathBuf::from(format!("{}-wal", path.display()));
        let size = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(size, 0, "TRUNCATE must leave an empty WAL");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&wal);
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }
}
