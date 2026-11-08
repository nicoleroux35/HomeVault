/* =============================================================================
 * File          : restore_drill.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Non-destructive bare-metal restore drill (P2-9). It proves the recovery
 *   chain without touching live data: copy the newest database snapshot into a
 *   temporary directory and run SQLite's integrity check on the copy, decode
 *   the newest configuration archive (checksum-verified), and report the last
 *   data-backup job. Nothing is restored in place and no pool is attached.
 *
 * Dependencies:
 *   - sqlx (SQLite), chrono, serde, tempfile
 *
 * Usage:
 *   `restore_drill::run(...)` behind POST /api/settings/backup/restore-drill.
 * =============================================================================
 */

//! Non-destructive restore drill.

use crate::config_backup::{self, ArchiveError};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::{Path, PathBuf};

/// Largest configuration archive the drill will read into memory. Exports are
/// tiny (settings + generated configs); anything larger is not a HomeVault
/// archive and must not be able to exhaust the process.
const MAX_CONFIG_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;

/// A data-backup job older than this is reported as a warning, not a blocker:
/// the operator may rely on replication or immutable snapshots instead.
const DATA_BACKUP_STALE_DAYS: i64 = 7;

/// One line of the drill report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrillCheck {
    /// Human label (`database`, `configuration`, `data backup`).
    pub name: String,
    /// File or job the check looked at, when there is one.
    pub source: Option<String>,
    /// The source exists and passed the checks we can run.
    pub ok: bool,
    /// The payload was fully verified (not just its structure).
    pub verified: bool,
    /// Sentence shown to the operator, including the reason on failure.
    pub detail: String,
}

impl DrillCheck {
    fn missing(name: &str, detail: &str) -> Self {
        Self {
            name: name.to_string(),
            source: None,
            ok: false,
            verified: false,
            detail: detail.to_string(),
        }
    }
}

/// The whole drill result, persisted so Settings can show the last run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreDrillReport {
    pub started_at: String,
    pub finished_at: String,
    /// True only when the database and configuration can both be restored.
    pub ready: bool,
    pub database: DrillCheck,
    pub config: DrillCheck,
    pub data_backup: DrillCheck,
    pub warnings: Vec<String>,
}

/// Runs the drill against explicit directories, so tests never depend on the
/// process-wide data directory.
pub async fn run(
    db_backup_dir: &Path,
    config_backup_dir: &Path,
    pool: &SqlitePool,
    passphrase: Option<&str>,
) -> RestoreDrillReport {
    let started_at = Utc::now().to_rfc3339();
    let tmp = match DrillTemp::new() {
        Ok(tmp) => tmp,
        Err(e) => {
            let failed = DrillCheck {
                name: "database".to_string(),
                source: None,
                ok: false,
                verified: false,
                detail: format!("could not create a temporary directory: {e}"),
            };
            return RestoreDrillReport {
                started_at: started_at.clone(),
                finished_at: Utc::now().to_rfc3339(),
                ready: false,
                database: failed,
                config: DrillCheck::missing(
                    "configuration",
                    "the drill could not start",
                ),
                data_backup: DrillCheck::missing("data backup", "the drill could not start"),
                warnings: Vec::new(),
            };
        }
    };

    let database = check_database(db_backup_dir, tmp.path()).await;
    let (config, config_warning) = check_config(config_backup_dir, passphrase);
    let (data_backup, data_warning) = check_data_backup(pool).await;

    let mut warnings = Vec::new();
    if let Some(warning) = config_warning {
        warnings.push(warning);
    }
    if let Some(warning) = data_warning {
        warnings.push(warning);
    }

    RestoreDrillReport {
        started_at,
        finished_at: Utc::now().to_rfc3339(),
        ready: ready(&database, &config),
        database,
        config,
        data_backup,
        warnings,
    }
}

/// Temporary directory removed on drop. `tempfile` is a dev-dependency only,
/// and the drill runs in the server process, so it owns this small guard.
struct DrillTemp(PathBuf);

impl DrillTemp {
    fn new() -> std::io::Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "homevault-restore-drill-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DrillTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A drill is "ready" only when both halves of the recovery chain were fully
/// verified. The data-backup line is advisory (replication/snapshots may cover
/// it), so it is reported but does not flip this flag.
fn ready(database: &DrillCheck, config: &DrillCheck) -> bool {
    database.ok && database.verified && config.ok && config.verified
}

async fn check_database(dir: &Path, tmp: &Path) -> DrillCheck {
    let Some(source) = newest_file(dir, "db") else {
        return DrillCheck::missing(
            "database",
            "no database snapshot found; take one from the Database snapshots card",
        );
    };
    if !has_sqlite_header(&source) {
        return DrillCheck {
            name: "database".to_string(),
            source: source.file_name().map(|n| n.to_string_lossy().to_string()),
            ok: false,
            verified: false,
            detail: "the snapshot is not a readable SQLite database (bad header)".to_string(),
        };
    }

    let copy = tmp.join("restore-drill-snapshot.db");
    if let Err(e) = std::fs::copy(&source, &copy) {
        return DrillCheck {
            name: "database".to_string(),
            source: source.file_name().map(|n| n.to_string_lossy().to_string()),
            ok: false,
            verified: false,
            detail: format!("the snapshot could not be copied for verification: {e}"),
        };
    }

    let options = SqliteConnectOptions::new()
        .filename(&copy)
        .read_only(true);
    let pool = match SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            return DrillCheck {
                name: "database".to_string(),
                source: source.file_name().map(|n| n.to_string_lossy().to_string()),
                ok: false,
                verified: false,
                detail: format!("the snapshot is not a readable SQLite database: {e}"),
            };
        }
    };

    let integrity = sqlx::query("PRAGMA integrity_check")
        .fetch_all(&pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .filter_map(|row| row.try_get::<String, _>(0).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|e| vec![format!("integrity_check failed: {e}")]);
    let integrity_ok = integrity.iter().all(|line| line == "ok");

    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap_or(-1);
    pool.close().await;

    let source_name = source.file_name().map(|n| n.to_string_lossy().to_string());
    if integrity_ok && tables > 0 {
        let users_text = if users >= 0 {
            format!("{users} user(s)")
        } else {
            "users table unreadable".to_string()
        };
        DrillCheck {
            name: "database".to_string(),
            source: source_name,
            ok: true,
            verified: true,
            detail: format!("integrity_check=ok, {tables} table(s), {users_text}"),
        }
    } else {
        DrillCheck {
            name: "database".to_string(),
            source: source_name,
            ok: false,
            verified: false,
            detail: if integrity_ok {
                "the snapshot has no application tables".to_string()
            } else {
                format!("integrity_check: {}", integrity.join("; "))
            },
        }
    }
}

fn check_config(dir: &Path, passphrase: Option<&str>) -> (DrillCheck, Option<String>) {
    let Some(source) = newest_file(dir, "hvbak") else {
        return (
            DrillCheck::missing(
                "configuration",
                "no configuration archive found; export one from Settings → Configuration backup",
            ),
            Some(
                "No configuration archive exists, so a bare-metal rebuild would lose accounts, shares and settings."
                    .to_string(),
            ),
        );
    };
    let source_name = source.file_name().map(|n| n.to_string_lossy().to_string());
    let metadata = std::fs::metadata(&source);
    if let Ok(metadata) = &metadata {
        if metadata.len() > MAX_CONFIG_ARCHIVE_BYTES {
            return (
                DrillCheck {
                    name: "configuration".to_string(),
                    source: source_name,
                    ok: false,
                    verified: false,
                    detail: format!(
                        "the archive is larger than {} MiB; refusing to read it",
                        MAX_CONFIG_ARCHIVE_BYTES / (1024 * 1024)
                    ),
                },
                None,
            );
        }
    }
    let bytes = match std::fs::read(&source) {
        Ok(bytes) => bytes,
        Err(e) => {
            return (
                DrillCheck {
                    name: "configuration".to_string(),
                    source: source_name,
                    ok: false,
                    verified: false,
                    detail: format!("the archive could not be read: {e}"),
                },
                None,
            );
        }
    };

    match config_backup::read(&bytes, passphrase) {
        Ok(archive) => {
            let manifest = &archive.manifest;
            (
                DrillCheck {
                    name: "configuration".to_string(),
                    source: source_name,
                    ok: true,
                    verified: true,
                    detail: format!(
                        "format v{}, {} entries, {} skipped, encrypted={}",
                        manifest.format_version,
                        manifest.entries.len(),
                        manifest.skipped.len(),
                        manifest.encrypted
                    ),
                },
                None,
            )
        }
        Err(ArchiveError::PassphraseRequired) => (
            DrillCheck {
                name: "configuration".to_string(),
                source: source_name,
                ok: true,
                verified: false,
                detail: "the archive is encrypted; enter its passphrase to verify the payload"
                    .to_string(),
            },
            Some(
                "The configuration archive is encrypted: its structure is valid but its payload was not verified."
                    .to_string(),
            ),
        ),
        Err(ArchiveError::NotEncrypted) => {
            // A passphrase was supplied for a plain archive: decode it without
            // the passphrase instead of failing the drill.
            match config_backup::read(&bytes, None) {
                Ok(archive) => (
                    DrillCheck {
                        name: "configuration".to_string(),
                        source: source_name,
                        ok: true,
                        verified: true,
                        detail: format!(
                            "format v{}, {} entries, {} skipped, encrypted=false",
                            archive.manifest.format_version,
                            archive.manifest.entries.len(),
                            archive.manifest.skipped.len()
                        ),
                    },
                    None,
                ),
                Err(e) => (
                    DrillCheck {
                        name: "configuration".to_string(),
                        source: source_name,
                        ok: false,
                        verified: false,
                        detail: e.message(),
                    },
                    None,
                ),
            }
        }
        Err(e) => (
            DrillCheck {
                name: "configuration".to_string(),
                source: source_name,
                ok: false,
                verified: false,
                detail: e.message(),
            },
            None,
        ),
    }
}

async fn check_data_backup(pool: &SqlitePool) -> (DrillCheck, Option<String>) {
    let row = sqlx::query(
        "SELECT status, finished_at, error_message FROM restic_backup_jobs \
         ORDER BY COALESCE(finished_at, started_at) DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await;

    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                DrillCheck {
                    name: "data backup".to_string(),
                    source: None,
                    ok: true,
                    verified: false,
                    detail: "no restic backup job recorded (replication or snapshots may still cover the data)"
                        .to_string(),
                },
                Some(
                    "No data backup was recorded: verify replication or restic separately before relying on this drill."
                        .to_string(),
                ),
            );
        }
        Err(e) => {
            return (
                DrillCheck {
                    name: "data backup".to_string(),
                    source: None,
                    ok: false,
                    verified: false,
                    detail: format!("the backup history could not be read: {e}"),
                },
                None,
            );
        }
    };

    let status: String = row.get("status");
    let finished_at: Option<String> = row.try_get("finished_at").ok();
    let error: Option<String> = row.try_get("error_message").ok();
    let fresh = finished_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| {
            Utc::now() - value.with_timezone(&Utc) <= chrono::Duration::days(DATA_BACKUP_STALE_DAYS)
        })
        .unwrap_or(false);

    if status == "success" && fresh {
        (
            DrillCheck {
                name: "data backup".to_string(),
                source: finished_at.clone(),
                ok: true,
                verified: true,
                detail: format!(
                    "last restic job succeeded at {}",
                    finished_at.unwrap_or_else(|| "an unknown time".to_string())
                ),
            },
            None,
        )
    } else if status == "success" {
        (
            DrillCheck {
                name: "data backup".to_string(),
                source: finished_at.clone(),
                ok: true,
                verified: false,
                detail: format!(
                    "the last restic job succeeded at {} but is older than {} days",
                    finished_at.unwrap_or_else(|| "an unknown time".to_string()),
                    DATA_BACKUP_STALE_DAYS
                ),
            },
            Some("The last data backup is older than the drill's freshness window.".to_string()),
        )
    } else {
        (
            DrillCheck {
                name: "data backup".to_string(),
                source: finished_at,
                ok: false,
                verified: false,
                detail: match error {
                    Some(error) => format!("the last restic job is '{status}': {error}"),
                    None => format!("the last restic job is '{status}'"),
                },
            },
            Some("The most recent data backup did not succeed.".to_string()),
        )
    }
}

/// True when the file starts with SQLite's 16-byte header. Checked before
/// opening so a truncated or foreign file is reported as "not a database"
/// instead of as an opaque connection error.
fn has_sqlite_header(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 16];
    file.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0"
}

/// Newest file with `extension` in `dir`, by modification time then name.
fn newest_file(dir: &Path, extension: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, String, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some(extension) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        match &best {
            Some((best_time, best_name, _))
                if (modified, &name) <= (*best_time, best_name) => {}
            _ => best = Some((modified, name, path)),
        }
    }
    best.map(|(_, _, path)| path)
}

/// Persists the last report under `<base>/restore-drill.json` (atomic write).
pub fn save_report(base_dir: &Path, report: &RestoreDrillReport) -> Result<(), String> {
    let path = base_dir.join("restore-drill.json");
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Reads the last report, if any.
pub fn load_report(base_dir: &Path) -> Option<RestoreDrillReport> {
    let body = std::fs::read(base_dir.join("restore-drill.json")).ok()?;
    serde_json::from_slice(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, ok: bool, verified: bool) -> DrillCheck {
        DrillCheck {
            name: name.to_string(),
            source: None,
            ok,
            verified,
            detail: String::new(),
        }
    }

    #[test]
    fn test_ready_requires_both_halves_verified() {
        assert!(ready(
            &check("database", true, true),
            &check("configuration", true, true)
        ));
        assert!(!ready(
            &check("database", true, false),
            &check("configuration", true, true)
        ));
        assert!(!ready(
            &check("database", true, true),
            &check("configuration", false, false)
        ));
    }

    #[test]
    fn test_newest_file_picks_the_latest_name_on_equal_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.db"), b"a").unwrap();
        std::fs::write(tmp.path().join("b.db"), b"b").unwrap();
        std::fs::write(tmp.path().join("ignore.txt"), b"x").unwrap();
        let picked = newest_file(tmp.path(), "db").unwrap();
        assert_eq!(picked.file_name().unwrap(), std::ffi::OsStr::new("b.db"));
    }

    #[test]
    fn test_newest_file_returns_none_for_an_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(newest_file(tmp.path(), "db").is_none());
    }

    #[test]
    fn test_missing_check_is_never_ready() {
        let report = RestoreDrillReport {
            started_at: "2026-11-08T00:00:00+00:00".to_string(),
            finished_at: "2026-11-08T00:00:01+00:00".to_string(),
            ready: ready(
                &DrillCheck::missing("database", "none"),
                &DrillCheck::missing("configuration", "none"),
            ),
            database: DrillCheck::missing("database", "none"),
            config: DrillCheck::missing("configuration", "none"),
            data_backup: DrillCheck::missing("data backup", "none"),
            warnings: Vec::new(),
        };
        assert!(!report.ready);
    }
}
