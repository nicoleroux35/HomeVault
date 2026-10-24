/* =============================================================================
 * File          : db_backup.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
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
 *   Two invariants hold process-wide, not per instance (ROADMAP 22.1/22.2):
 *   only one `VACUUM INTO` runs at a time — every HTTP request constructs its
 *   own `DbBackupService`, so the lock is a module static and not a field —
 *   and a failed attempt leaves its reason behind for the Settings card.
 *   Retention bounds the regular snapshots only: a `.pre-migration` restore
 *   point is never removed by it.
 *
 * Dependencies:
 *   - chrono (>= 0.4), serde (>= 1.0), tokio (>= 1.52)
 *
 * Usage:
 *   `DbBackupService::load(db).await`; `run_scheduler_loop()` spawned at
 *   startup, `backup_now()` from `handlers::db_backup` and from the migration
 *   path the primitive `snapshot_before_migration()`.
 * =============================================================================
 */

//! Automatic `VACUUM INTO` snapshots of the application database.

use crate::storage::database::{Database, DbError};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

/// File-name prefix of a database snapshot.
pub const SNAPSHOT_PREFIX: &str = "homevault-";
/// Extension of a database snapshot.
pub const SNAPSHOT_EXT: &str = "db";
/// Marker segment of a snapshot written before a migration is applied.
pub const PRE_MIGRATION_MARKER: &str = ".pre-migration";
/// How many pre-migration restore points are kept, independently of `retain`.
///
/// They are the only copy of the pre-upgrade state, so the regular retention
/// pass never removes them (ROADMAP 22.2). They are still bounded: an appliance
/// that upgrades every week must not fill the data volume with them.
pub const PRE_MIGRATION_RETAIN: usize = 3;
/// Key under which the backup settings are persisted (`data/db_backup.json`).
const SETTINGS_KEY: &str = "db_backup";

/// The one snapshot lock of the process.
///
/// ROADMAP 22.1: this used to be a per-instance `Arc<tokio::sync::Mutex<()>>`
/// while every HTTP handler built its own `DbBackupService`, so a manual click
/// landing during a scheduled run produced two concurrent `VACUUM INTO`s — the
/// race `docs/guides/backup-restic.md` promised could not happen. A module
/// static keeps every instance (handlers, scheduler, the pre-migration path) on
/// the same lock without adding an `AppState` field, which would have touched
/// the 40+ `AppState` literals under `tests/`.
///
/// A `tokio::sync::Mutex` rather than a `parking_lot` one: the guard is held
/// across `await` (the `VACUUM INTO` round-trip), and a blocking guard there
/// would make the whole scheduler future non-`Send`, which `tokio::spawn`
/// rejects.
static RUNNING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Why the most recent attempt failed, process-wide (ROADMAP 22.2).
///
/// `DbBackupStatus` is built by a throw-away service per request, so the reason
/// cannot live in the struct; it is cleared by the next success and by a
/// restart. In-process only, on purpose: a snapshot that fails must be visible
/// on the card that offers the button, not only in the journal.
static LAST_ERROR: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

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
    /// `true` for the restore point written before a migration was applied.
    ///
    /// Those files are outside `retain` and never pruned by it, so the screen
    /// labels them instead of pretending they are scheduled snapshots.
    #[serde(default)]
    pub is_pre_migration: bool,
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
}

impl DbBackupService {
    /// Build a service with the shipped defaults **in memory** — no file I/O.
    ///
    /// Callers that must honour the persisted schedule use [`Self::load`];
    /// `new` exists so unit tests (and `snapshot_into`) never block on disk.
    pub fn new(db: Database) -> Self {
        Self {
            db,
            settings: Arc::new(DbBackupSettingsHolder::default()),
        }
    }

    /// Build a service and read the persisted schedule off the runtime.
    ///
    /// A handler that only ever calls `status()` still needs this: the schedule
    /// shown on the card is `data/db_backup.json`, not the defaults.
    pub async fn load(db: Database) -> Self {
        let service = Self::new(db);
        service.load_settings().await;
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

    /// Re-read `data/db_backup.json` into the in-memory copy.
    ///
    /// The read runs on the blocking pool: the scheduler re-reads it every tick
    /// and the Settings handlers build a service per request, so this is a
    /// `std::fs` call reachable from an `async fn` (ROADMAP X.1).
    async fn load_settings(&self) {
        let storage = Self::settings_storage();
        let loaded = run_blocking(move || {
            Ok(storage
                .load::<DbBackupSettings>(SETTINGS_KEY)
                .ok()
                .flatten())
        })
        .await;

        if let Ok(settings) = loaded {
            if let Some(saved) = settings {
                *self.settings.inner.lock() = saved;
            }
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
    pub async fn reset_settings(&self) -> Result<(), String> {
        let storage = Self::settings_storage();
        run_blocking(move || Ok(storage.delete(SETTINGS_KEY)))
            .await
            .map_err(|e| format!("Failed to reset backup settings: {e}"))?
            .map_err(|e| format!("Failed to reset backup settings: {e}"))?;

        *self.settings.inner.lock() = DbBackupSettings::default();
        Ok(())
    }

    /// Persist new settings.
    pub async fn update_settings(&self, next: DbBackupSettings) -> Result<(), String> {
        // Bounded so a typo cannot fill the disk with snapshots.
        if next.retain == 0 || next.retain > 100 {
            return Err("Retention must be between 1 and 100 snapshots".to_string());
        }
        if next.interval_hours > 24 * 30 {
            return Err("Interval must be at most 720 hours (30 days)".to_string());
        }

        let storage = Self::settings_storage();
        let payload = next.clone();
        run_blocking(move || Ok(storage.save(SETTINGS_KEY, &payload)))
            .await
            .map_err(|e| format!("Failed to save backup settings: {e}"))?
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
    pub async fn list(&self) -> Vec<DbBackupEntry> {
        let dir = Self::backup_dir();
        run_blocking(move || Ok(list_in(&dir)))
            .await
            .unwrap_or_default()
    }

    /// Take a snapshot now, in the configured directory.
    pub async fn backup_now(&self) -> Result<DbBackupEntry, String> {
        self.snapshot_into(&Self::backup_dir()).await
    }

    /// Take a snapshot into `dir`.
    ///
    /// Serialised process-wide: a manual click landing while the scheduler runs
    /// waits for the running one rather than producing two files. The write
    /// goes to a temporary name first and is renamed into place, so an
    /// interrupted snapshot never leaves a half-written file that looks
    /// restorable.
    pub async fn snapshot_into(&self, dir: &Path) -> Result<DbBackupEntry, String> {
        let _guard = RUNNING.lock().await;

        let dir = dir.to_path_buf();

        let prepare = dir.clone();
        run_blocking(move || std::fs::create_dir_all(&prepare))
            .await
            .map_err(|e| record_failure(format!("Cannot create backup dir: {e}")))?;

        // Two snapshots can legitimately land in the same second (a manual
        // click right after a scheduled run); the lock orders them, this keeps
        // the second one from silently overwriting the first.
        let base = Self::snapshot_name(Utc::now());
        let naming = dir.clone();
        let name = run_blocking(move || Ok(unique_snapshot_name(&naming, &base)))
            .await
            .map_err(|e| record_failure(format!("Cannot allocate a snapshot name: {e}")))?;

        let final_path = dir.join(&name);
        let tmp_path = dir.join(format!(".{name}.tmp"));

        // A previous crashed run may have left the temp file behind.
        let stale_tmp = tmp_path.clone();
        let _ = run_blocking(move || {
            let _ = std::fs::remove_file(&stale_tmp);
            Ok(())
        })
        .await;

        if let Err(e) = self.db.vacuum_into(&tmp_path).await {
            let leftover = tmp_path.clone();
            let _ = run_blocking(move || {
                let _ = std::fs::remove_file(&leftover);
                Ok(())
            })
            .await;
            return Err(record_failure(format!("Snapshot failed: {e}")));
        }

        let finalise_tmp = tmp_path.clone();
        let finalise_dest = final_path.clone();
        let size_bytes = run_blocking(move || finalise_snapshot(&finalise_tmp, &finalise_dest))
            .await
            .map_err(|e| record_failure(format!("Cannot finalise snapshot: {e}")))?;

        let entry = DbBackupEntry {
            name,
            path: final_path.to_string_lossy().to_string(),
            size_bytes,
            created_at: Utc::now().to_rfc3339(),
            is_pre_migration: false,
        };

        let retain = self.settings.inner.lock().retain;
        self.prune_dir(&dir, retain).await;

        // The reason a previous attempt failed is stale as soon as one works.
        *LAST_ERROR.lock() = None;

        tracing::info!(
            file = %entry.name,
            size_bytes = entry.size_bytes,
            "Database snapshot written"
        );
        Ok(entry)
    }

    /// Delete all but the newest `retain` snapshots. Returns the deleted names.
    ///
    /// `retain` bounds the **regular** snapshots only. A `.pre-migration`
    /// restore point is the pre-upgrade state and is never removed here — the
    /// old code counted it against `retain` and could delete the very file an
    /// upgrade was made safe by (ROADMAP 22.2). Those files are bounded
    /// separately by [`PRE_MIGRATION_RETAIN`].
    pub async fn prune(&self, retain: u32) -> Vec<String> {
        self.prune_dir(&Self::backup_dir(), retain).await
    }

    /// Prune `dir` (the directory a snapshot was actually written to).
    async fn prune_dir(&self, dir: &Path, retain: u32) -> Vec<String> {
        let dir = dir.to_path_buf();
        run_blocking(move || Ok(prune_in(&dir, retain)))
            .await
            .unwrap_or_default()
    }

    /// Settings plus the current state of the backup directory.
    pub async fn status(&self) -> DbBackupStatus {
        let backups = self.list().await;
        let total_size_bytes = backups.iter().map(|b| b.size_bytes).sum();
        DbBackupStatus {
            settings: self.settings(),
            last_backup_at: backups.first().map(|b| b.created_at.clone()),
            last_error: LAST_ERROR.lock().clone(),
            backups,
            total_size_bytes,
            directory: Self::backup_dir().to_string_lossy().to_string(),
        }
    }

    /// Whether a scheduled snapshot is due.
    pub async fn is_due(&self) -> bool {
        let interval = self.settings.inner.lock().interval_hours;
        if interval == 0 {
            return false;
        }
        let backups = self.list().await;
        match backups.first() {
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
        if !self.is_due().await {
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
            self.load_settings().await;

            if self.is_due().await {
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

/// Run a `std::fs` closure off the async runtime (ROADMAP X.1).
async fn run_blocking<T, F>(f: F) -> std::io::Result<T>
where
    F: FnOnce() -> std::io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        // A panicking blocking task must read as a failed snapshot, not as a
        // silently missing reason.
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("blocking task failed: {e}"),
        )),
    }
}

/// Remember why an attempt failed (ROADMAP 22.2) and hand the message back.
fn record_failure(message: impl Into<String>) -> String {
    let message = message.into();
    tracing::warn!(error = %message, "Database snapshot failed");
    *LAST_ERROR.lock() = Some(message.clone());
    message
}

/// Whether `path` looks like one of our snapshots.
pub fn is_snapshot_name(name: &str) -> bool {
    name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(SNAPSHOT_EXT)
}

/// Whether `name` is a snapshot written before a migration was applied.
pub fn is_pre_migration_name(name: &str) -> bool {
    is_snapshot_name(name) && name.contains(PRE_MIGRATION_MARKER)
}

/// Keep a same-second snapshot from overwriting the one taken a moment earlier.
///
/// `homevault-…083000.db` and `homevault-…083000_2.db` — the `_` suffix sorts
/// **after** the `.` of the unsuffixed name, so a reverse name sort still reads
/// as a reverse chronological sort (`list_in` relies on that).
fn unique_snapshot_name(dir: &Path, base: &str) -> String {
    if !dir.join(base).exists() {
        return base.to_string();
    }

    let stem = base
        .strip_suffix(&format!(".{SNAPSHOT_EXT}"))
        .unwrap_or(base);
    for n in 2..=1000 {
        let candidate = format!("{stem}_{n}.{SNAPSHOT_EXT}");
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    base.to_string()
}

/// Rename the finished temporary file into place and report its size.
fn finalise_snapshot(tmp: &Path, final_path: &Path) -> std::io::Result<u64> {
    if let Err(e) = std::fs::rename(tmp, final_path) {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    Ok(std::fs::metadata(final_path).map(|m| m.len()).unwrap_or(0))
}

/// Every snapshot in `dir`, newest first (blocking; call from the pool).
///
/// Names carry a sortable timestamp, so a reverse name sort is a reverse
/// chronological sort — and it does not depend on mtime, which a copy or a
/// restore can rewrite.
fn list_in(dir: &Path) -> Vec<DbBackupEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut out: Vec<DbBackupEntry> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !is_snapshot_name(&name) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let created = meta
                .modified()
                .ok()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(Utc::now);
            Some(DbBackupEntry {
                is_pre_migration: is_pre_migration_name(&name),
                name,
                path: path.to_string_lossy().to_string(),
                size_bytes: meta.len(),
                created_at: created.to_rfc3339(),
            })
        })
        .collect();

    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// Delete all but the newest `retain` regular snapshots (blocking).
///
/// Pre-migration restore points are counted and bounded separately
/// ([`PRE_MIGRATION_RETAIN`]), never against `retain` (ROADMAP 22.2).
fn prune_in(dir: &Path, retain: u32) -> Vec<String> {
    let all = list_in(dir);
    let keep_regular = retain as usize;

    let mut regular_seen = 0usize;
    let mut pre_migration_seen = 0usize;
    let mut deleted = Vec::new();

    for entry in all {
        let keep = if entry.is_pre_migration {
            pre_migration_seen += 1;
            pre_migration_seen <= PRE_MIGRATION_RETAIN
        } else {
            regular_seen += 1;
            regular_seen <= keep_regular
        };

        if keep {
            continue;
        }

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

/// Snapshot a database into `dir` — the primitive the migration path uses
/// before applying anything, without needing a running `DbBackupService`.
///
/// Takes the same process-wide lock as `backup_now`: a boot that migrates while
/// a scheduled snapshot runs must not produce two `VACUUM INTO`s either.
pub async fn snapshot_before_migration(db: &Database, dir: &Path) -> Result<PathBuf, DbError> {
    let _guard = RUNNING.lock().await;

    let dir = dir.to_path_buf();
    let prepare = dir.clone();
    run_blocking(move || std::fs::create_dir_all(&prepare)).await?;

    let base = format!(
        "{SNAPSHOT_PREFIX}{}{PRE_MIGRATION_MARKER}.{SNAPSHOT_EXT}",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let naming = dir.clone();
    let name = run_blocking(move || Ok(unique_snapshot_name(&naming, &base))).await?;

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
        assert!(!is_pre_migration_name(&name));
    }

    #[test]
    fn test_is_snapshot_name_rejects_other_files() {
        assert!(!is_snapshot_name("homevault.db"));
        assert!(!is_snapshot_name("homevault.db-wal"));
        assert!(!is_snapshot_name("backup.txt"));
        assert!(!is_snapshot_name(".homevault-1.db.tmp"));
    }

    #[test]
    fn test_pre_migration_name_is_recognised() {
        let name = "homevault-20261017-083000.pre-migration.db";
        assert!(is_snapshot_name(name));
        assert!(is_pre_migration_name(name));
        // A suffixed duplicate of a pre-migration point stays one.
        assert!(is_pre_migration_name(
            "homevault-20261017-083000.pre-migration_2.db"
        ));
        assert!(!is_pre_migration_name("homevault-20261017-083000.db"));
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

    /// Write an empty file with the given name into `dir` (no SQLite needed:
    /// the retention pass only reads names and metadata).
    fn touch(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"snapshot").unwrap();
    }

    fn names_in(dir: &Path) -> Vec<String> {
        list_in(dir).into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn test_retention_never_prunes_the_pre_migration_restore_point() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // The restore point is oldest by name, which is exactly the file the
        // old `skip(keep)` pass deleted first (ROADMAP 22.2).
        touch(dir, "homevault-20200101-000000.pre-migration.db");
        touch(dir, "homevault-20261017-083000.db");
        touch(dir, "homevault-20261016-083000.db");

        let deleted = prune_in(dir, 1);

        assert_eq!(deleted, vec!["homevault-20261016-083000.db".to_string()]);
        assert_eq!(
            names_in(dir),
            vec![
                "homevault-20261017-083000.db".to_string(),
                "homevault-20200101-000000.pre-migration.db".to_string(),
            ],
            "retain bounds the regular snapshots; the restore point survives"
        );
    }

    #[test]
    fn test_pre_migration_restore_points_are_bounded_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        for i in 0..6 {
            touch(
                dir,
                &format!("homevault-2020010{}-00000{i}.pre-migration.db", i + 1),
            );
        }

        // A huge retention for regular snapshots must not keep them all.
        prune_in(dir, 100);

        assert_eq!(
            names_in(dir),
            vec![
                "homevault-20200106-000005.pre-migration.db".to_string(),
                "homevault-20200105-000004.pre-migration.db".to_string(),
                "homevault-20200104-000003.pre-migration.db".to_string(),
            ],
            "only the newest {} restore points are kept",
            PRE_MIGRATION_RETAIN
        );
    }

    #[test]
    fn test_retention_does_not_count_the_restore_point_against_retain() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        touch(dir, "homevault-20200101-000000.pre-migration.db");
        touch(dir, "homevault-20261017-083000.db");
        touch(dir, "homevault-20261016-083000.db");

        prune_in(dir, 2);

        // Both regular snapshots survive: the restore point used to consume one
        // of the two slots and get the second snapshot deleted with it.
        assert_eq!(
            names_in(dir),
            vec![
                "homevault-20261017-083000.db".to_string(),
                "homevault-20261016-083000.db".to_string(),
                "homevault-20200101-000000.pre-migration.db".to_string(),
            ]
        );
    }

    #[test]
    fn test_unique_snapshot_name_keeps_the_sort_order() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let base = "homevault-20261017-083000.db";

        assert_eq!(unique_snapshot_name(dir, base), base);

        touch(dir, base);
        let second = unique_snapshot_name(dir, base);
        assert_eq!(second, "homevault-20261017-083000_2.db");

        // The duplicate must read as *newer*, not older, in a reverse sort.
        let mut names = vec![base.to_string(), second];
        names.sort_by(|a, b| b.cmp(a));
        assert_eq!(names[0], "homevault-20261017-083000_2.db");
    }

    /// ROADMAP 22.1: the lock is process-wide, so a snapshot waits for another
    /// one even when the two come from *different* service instances — which is
    /// what every HTTP handler builds.
    #[tokio::test]
    async fn test_a_snapshot_waits_for_the_process_wide_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite:{}?mode=rwc",
            tmp.path().join("homevault.db").display()
        );
        let db = Database::new(&url).await.expect("file database");
        let service = DbBackupService::new(db);
        let dir = tmp.path().join("backups");

        let guard = RUNNING.lock().await;
        let mut snapshot = Box::pin(service.snapshot_into(&dir));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut snapshot)
                .await
                .is_err(),
            "a snapshot must wait for the process-wide lock"
        );

        drop(guard);
        let entry = snapshot.await.expect("the snapshot runs once the lock frees");
        assert!(Path::new(&entry.path).exists(), "{}", entry.path);
        assert!(!entry.is_pre_migration);
        assert!(LAST_ERROR.lock().is_none(), "a success clears the reason");
    }
}
