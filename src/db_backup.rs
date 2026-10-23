/* =============================================================================
 * File          : db_backup.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 17/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Automatic snapshots of HomeVault's own SQLite database.
 *
 *   The database runs in WAL mode, so the `.db` file on its own is not a
 *   backup: every committed transaction may still sit in `-wal`, and copying
 *   just `homevault.db` yields a file with no tables at all. Snapshots here
 *   are produced with `VACUUM INTO`, which writes a single self-contained file
 *   — copying one of those is enough to restore the appliance.
 *
 *   A snapshot is taken at startup when the last one is older than the
 *   configured interval, daily afterwards, and on demand from Settings. A
 *   snapshot is also forced before any pending migration is applied, so an
 *   upgrade always has a point to come back to.
 *
 * Dependencies:
 *   - chrono (>= 0.4), serde (>= 1.0), tokio (>= 1.52)
 *
 * Usage:
 *   `DbBackupService::new(db)`; `run_scheduler_loop()` spawned at startup,
 *   `backup_now()` from `handlers::db_backup` and from the migration path.
 * =============================================================================
 */

//! Automatic `VACUUM INTO` snapshots of the application database.

use crate::storage::database::{Database, DbError};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// File-name prefix of a database snapshot.
pub const SNAPSHOT_PREFIX: &str = "homevault-";
/// Extension of a database snapshot.
pub const SNAPSHOT_EXT: &str = "db";
/// Key under which the backup settings are persisted (`data/db_backup.json`).
const SETTINGS_KEY: &str = "db_backup";

/// How often snapshots are taken, and how many are kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbBackupSettings {
    /// Snapshot automatically every N hours. `0` disables the schedule
    /// (the manual button still works).
    pub interval_hours: u32,
    /// How many snapshots to keep. Older ones are pruned after each run.
    pub retain: u32,
}

impl Default for DbBackupSettings {
    fn default() -> Self {
        Self {
            interval_hours: 24,
            retain: 7,
        }
    }
}

/// One snapshot on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbBackupEntry {
    /// File name (`homevault-20261017-083000.db`).
    pub name: String,
    /// Absolute path on disk.
    pub path: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Creation time, ISO-8601 (UTC).
    pub created_at: String,
}

/// Runtime status surfaced to Settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbBackupStatus {
    pub settings: DbBackupSettings,
    /// When the most recent successful snapshot was taken.
    pub last_backup_at: Option<String>,
    /// Error from the most recent attempt, cleared on success.
    pub last_error: Option<String>,
    /// Snapshots currently on disk, newest first.
    pub backups: Vec<DbBackupEntry>,
    /// Total size of the snapshots on disk.
    pub total_size_bytes: u64,
    /// Directory the snapshots live in.
    pub directory: String,
}

/// Interior-mutable holder so the scheduler and the HTTP handler can share one
/// set of settings without threading an `Arc<RwLock<_>>` through `AppState`.
#[derive(Debug, Default)]
struct DbBackupSettingsHolder {
    inner: Mutex<DbBackupSettings>,
}

/// Database snapshot service.
#[derive(Clone)]
pub struct DbBackupService {
    db: Database,
    settings: Arc<DbBackupSettingsHolder>,
    /// Guards against two snapshots running at once (scheduler + manual click).
    ///
    /// A `tokio::sync::Mutex` rather than a `parking_lot` one: the guard is held
    /// across `await` (the `VACUUM INTO` round-trip), and a blocking guard there
    /// would make the whole scheduler future non-`Send`, which `tokio::spawn`
    /// rejects.
    running: Arc<tokio::sync::Mutex<()>>,
}

impl DbBackupService {
    pub fn new(db: Database) -> Self {
        let service = Self {
            db,
            settings: Arc::new(DbBackupSettingsHolder::default()),
            running: Arc::new(tokio::sync::Mutex::new(())),
        };
        service.load_settings();
        service
    }

    /// Directory holding the snapshots (`{DATA_DIRECTORY}/data/backups`).
    pub fn backup_dir() -> PathBuf {
        crate::config::paths::db_backup_dir()
    }

    fn settings_storage() -> crate::storage::json::JsonStorage {
        crate::storage::json::JsonStorage::new(
            crate::config::paths::base_dir()
                .to_string_lossy()
                .to_string(),
        )
    }

    fn load_settings(&self) {
        if let Ok(Some(saved)) = Self::settings_storage().load::<DbBackupSettings>(SETTINGS_KEY) {
            *self.settings.inner.lock() = saved;
        }
    }

    /// Current settings (a copy).
    pub fn settings(&self) -> DbBackupSettings {
        self.settings.inner.lock().clone()
    }

    /// Reset the stored schedule to the shipped defaults.
    ///
    /// ROADMAP 13.5: "Reset to Defaults" left `data/db_backup.json` behind, so
    /// the hourly schedule an admin had configured survived the reset. The file
    /// is deleted rather than rewritten, so a later release's default applies
    /// without a migration.
    pub fn reset_settings(&self) -> Result<(), String> {
        match Self::settings_storage().delete(SETTINGS_KEY) {
            Ok(()) => {
                *self.settings.inner.lock() = DbBackupSettings::default();
                Ok(())
            }
            Err(e) => Err(format!("Failed to reset backup settings: {e}")),
        }
    }

    /// Persist new settings.
    pub fn update_settings(&self, next: DbBackupSettings) -> Result<(), String> {
        // Bounded so a typo cannot fill the disk with snapshots.
        if next.retain == 0 || next.retain > 100 {
            return Err("Retention must be between 1 and 100 snapshots".to_string());
        }
        if next.interval_hours > 24 * 30 {
            return Err("Interval must be at most 720 hours (30 days)".to_string());
        }
        Self::settings_storage()
            .save(SETTINGS_KEY, &next)
            .map_err(|e| format!("Failed to save backup settings: {e}"))?;
        *self.settings.inner.lock() = next;
        Ok(())
    }

    /// Snapshot file name for a given instant.
    fn snapshot_name(at: DateTime<Utc>) -> String {
        format!(
            "{SNAPSHOT_PREFIX}{}.{SNAPSHOT_EXT}",
            at.format("%Y%m%d-%H%M%S")
        )
    }

    /// Every snapshot on disk, newest first.
    pub fn list(&self) -> Vec<DbBackupEntry> {
        let dir = Self::backup_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };

        let mut out: Vec<DbBackupEntry> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?.to_string();
                if !name.starts_with(SNAPSHOT_PREFIX) || !name.ends_with(SNAPSHOT_EXT) {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                let created = meta
                    .modified()
                    .ok()
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(Utc::now);
                Some(DbBackupEntry {
                    name,
                    path: path.to_string_lossy().to_string(),
                    size_bytes: meta.len(),
                    created_at: created.to_rfc3339(),
                })
            })
            .collect();

        // Names carry a sortable timestamp, so a reverse name sort is a
        // reverse chronological sort — and it does not depend on mtime, which a
        // copy or a restore can rewrite.
        out.sort_by(|a, b| b.name.cmp(&a.name));
        out
    }

    /// Take a snapshot now.
    ///
    /// Serialised: a manual click landing while the scheduler runs waits for
    /// the running one rather than producing two files. The write goes to a
    /// temporary name first and is renamed into place, so an interrupted
    /// snapshot never leaves a half-written file that looks restorable.
    pub async fn backup_now(&self) -> Result<DbBackupEntry, String> {
        let _guard = self.running.lock().await;

        let dir = Self::backup_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create backup dir: {e}"))?;

        let name = Self::snapshot_name(Utc::now());
        let final_path = dir.join(&name);
        let tmp_path = dir.join(format!(".{name}.tmp"));

        // A previous crashed run may have left the temp file behind.
        let _ = std::fs::remove_file(&tmp_path);

        self.db
            .vacuum_into(&tmp_path)
            .await
            .map_err(|e| format!("Snapshot failed: {e}"))?;

        std::fs::rename(&tmp_path, &final_path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            format!("Cannot finalise snapshot: {e}")
        })?;

        let size_bytes = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
        let entry = DbBackupEntry {
            name,
            path: final_path.to_string_lossy().to_string(),
            size_bytes,
            created_at: Utc::now().to_rfc3339(),
        };

        let retain = self.settings.inner.lock().retain;
        self.prune(retain);

        tracing::info!(
            file = %entry.name,
            size_bytes = entry.size_bytes,
            "Database snapshot written"
        );
        Ok(entry)
    }

    /// Delete all but the newest `retain` snapshots. Returns the deleted names.
    pub fn prune(&self, retain: u32) -> Vec<String> {
        let all = self.list();
        let keep = retain as usize;
        if all.len() <= keep {
            return Vec::new();
        }

        let mut deleted = Vec::new();
        for entry in all.into_iter().skip(keep) {
            match std::fs::remove_file(&entry.path) {
                Ok(()) => {
                    tracing::info!(file = %entry.name, "Pruned old database snapshot");
                    deleted.push(entry.name);
                }
                Err(e) => tracing::warn!(
                    file = %entry.name,
                    error = %e,
                    "Could not prune a database snapshot"
                ),
            }
        }
        deleted
    }

    /// Settings plus the current state of the backup directory.
    pub fn status(&self) -> DbBackupStatus {
        let backups = self.list();
        let total_size_bytes = backups.iter().map(|b| b.size_bytes).sum();
        DbBackupStatus {
            settings: self.settings(),
            last_backup_at: backups.first().map(|b| b.created_at.clone()),
            last_error: None,
            backups,
            total_size_bytes,
            directory: Self::backup_dir().to_string_lossy().to_string(),
        }
    }

    /// Whether a scheduled snapshot is due.
    pub fn is_due(&self) -> bool {
        let interval = self.settings.inner.lock().interval_hours;
        if interval == 0 {
            return false;
        }
        match self.list().first() {
            None => true,
            Some(newest) => {
                let Ok(created) = DateTime::parse_from_rfc3339(&newest.created_at) else {
                    return true;
                };
                let age = Utc::now().signed_duration_since(created.with_timezone(&Utc));
                age.num_hours() >= interval as i64
            }
        }
    }

    /// Snapshot at startup when due, never failing the boot.
    pub async fn backup_if_due(&self) {
        if !self.is_due() {
            return;
        }
        if let Err(e) = self.backup_now().await {
            tracing::warn!(error = %e, "Startup database snapshot failed");
        }
    }

    /// Daily scheduler loop: snapshot when due, fold the WAL back, repeat.
    pub async fn run_scheduler_loop(self) {
        // Give the server a moment to finish starting before touching the file.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;

        loop {
            // Re-read the persisted schedule every tick: the Settings reset
            // deletes the file, and each handler builds its own instance, so a
            // copy loaded once at startup would keep the old interval until the
            // next restart (ROADMAP 13.5).
            self.load_settings();

            if self.is_due() {
                if let Err(e) = self.backup_now().await {
                    tracing::warn!(error = %e, "Scheduled database snapshot failed");
                }
            }

            // Keep the write-ahead log bounded: an unbounded `-wal` is a
            // disk-space hazard and makes the `.db` file look empty to anyone
            // who copies it.
            if let Err(e) = self.db.wal_checkpoint().await {
                tracing::debug!(error = %e, "WAL checkpoint skipped");
            }

            tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
        }
    }
}

/// Whether `path` looks like one of our snapshots.
pub fn is_snapshot_name(name: &str) -> bool {
    name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(SNAPSHOT_EXT)
}

/// Snapshot a database into `dir` — the primitive the migration path uses
/// before applying anything, without needing a running `DbBackupService`.
pub async fn snapshot_before_migration(db: &Database, dir: &Path) -> Result<PathBuf, DbError> {
    std::fs::create_dir_all(dir)?;
    let name = format!(
        "{SNAPSHOT_PREFIX}{}.pre-migration.{SNAPSHOT_EXT}",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let target = dir.join(name);
    db.vacuum_into(&target).await?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_name_shape() {
        let at = DateTime::parse_from_rfc3339("2026-10-17T08:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let name = DbBackupService::snapshot_name(at);
        assert_eq!(name, "homevault-20261017-083000.db");
        assert!(is_snapshot_name(&name));
    }

    #[test]
    fn test_is_snapshot_name_rejects_other_files() {
        assert!(!is_snapshot_name("homevault.db"));
        assert!(!is_snapshot_name("homevault.db-wal"));
        assert!(!is_snapshot_name("backup.txt"));
        assert!(!is_snapshot_name(".homevault-1.db.tmp"));
    }

    #[test]
    fn test_default_settings() {
        let s = DbBackupSettings::default();
        assert_eq!(s.interval_hours, 24);
        assert_eq!(s.retain, 7);
    }

    /// The retention window `update_settings` enforces: 1..=100 snapshots.
    fn retention_is_valid(retain: u32) -> bool {
        (1..=100).contains(&retain)
    }

    #[test]
    fn test_settings_reject_out_of_range_retention() {
        assert!(retention_is_valid(1));
        assert!(retention_is_valid(7));
        assert!(retention_is_valid(100));
        assert!(!retention_is_valid(0));
        assert!(!retention_is_valid(101));
    }

    #[test]
    fn test_snapshot_names_sort_newest_first() {
        let mut names = [
            "homevault-20261015-083000.db".to_string(),
            "homevault-20261017-083000.db".to_string(),
            "homevault-20261016-083000.db".to_string(),
        ];
        names.sort_by(|a, b| b.cmp(a));
        assert_eq!(names[0], "homevault-20261017-083000.db");
        assert_eq!(names[2], "homevault-20261015-083000.db");
    }
}
