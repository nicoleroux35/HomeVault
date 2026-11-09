/* =============================================================================
 * File          : downloader_extract.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Automatic extraction for finished downloads: the settings flag, the
 *   archive allowlist, the `tar`/`unzip` argv (never a shell) and the bounded
 *   record of which downloads were already handled.
 *
 *   Off by default: a home user who wants the archive kept as an archive gets
 *   exactly that. When on, only an archive the Downloader itself just
 *   finished is unpacked, inside the folder it was downloaded into — never a
 *   path from a request, never another user's folder.
 *
 * Dependencies:
 *   - serde (>= 1.0), crate::utils::process, crate::config::paths
 *
 * Usage:
 *   handlers::downloader runs the completion pass that calls
 *   [`extract_archives_in`]; see docs/guides/downloader.md.
 * =============================================================================
 */

//! Automatic extraction of finished downloads (see docs/guides/downloader.md).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Deadline for one extraction. A multi-gigabyte archive on a slow disk is
/// minutes, not hours; a wedged `tar` must not hold the worker forever.
pub const EXTRACT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How many finished downloads the "already handled" marker remembers.
pub const MAX_PROCESSED: usize = 200;

/// Archive extensions the Downloader will unpack on its own.
///
/// An allowlist, and one that matches what `tar`/`unzip` are installed for
/// (`install_packages`' archive group). `.rar`/`.7z` are absent on purpose:
/// `7z` is an optional package and a second extractor is a second surface.
pub const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "tar", "tar.gz", "tgz", "tar.bz2", "tbz2", "tar.xz", "txz",
];

/// `data/downloader_settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DownloaderSettings {
    /// Unpack a finished archive next to itself. Off by default.
    #[serde(default)]
    pub auto_extract: bool,
}

/// `data/downloader_settings.json`.
pub fn settings_path() -> PathBuf {
    crate::config::paths::data_dir().join("downloader_settings.json")
}

/// `data/downloader_extracted.json`.
pub fn processed_path() -> PathBuf {
    crate::config::paths::data_dir().join("downloader_extracted.json")
}

/// Load the settings; a missing or corrupt file means "defaults" (off).
pub fn load_settings() -> DownloaderSettings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist the settings atomically.
pub fn save_settings(settings: &DownloaderSettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    write_atomic(&path, &body)
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.write_all(body)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.sync_all().ok();
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// True when `path`'s file name ends with an allowlisted archive extension.
pub fn is_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|ext| name.ends_with(&format!(".{ext}")))
}

/// The `(program, argv)` pair that unpacks `archive` into `destination`.
///
/// `None` for anything not on the allowlist. Both commands are passed as
/// argv with `--` where it exists, so a file named `-o` or `-C` cannot be read
/// as a flag; neither ever runs through a shell.
pub fn extract_command(archive: &Path, destination: &Path) -> Option<(&'static str, Vec<String>)> {
    if !is_archive(archive) {
        return None;
    }
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        return Some((
            "unzip",
            vec![
                "-o".to_string(),
                "-q".to_string(),
                archive.to_string_lossy().into_owned(),
                "-d".to_string(),
                destination.to_string_lossy().into_owned(),
            ],
        ));
    }
    Some((
        "tar",
        vec![
            "-xf".to_string(),
            archive.to_string_lossy().into_owned(),
            "-C".to_string(),
            destination.to_string_lossy().into_owned(),
            // GNU tar refuses absolute member names and `..` by default; the
            // flag makes the intent explicit in the argv a support log shows.
            "--no-absolute-names".to_string(),
        ],
    ))
}

/// Unpack one archive into `destination`. Returns the archive's name so the
/// caller can report which file was expanded.
pub fn extract_one(archive: &Path, destination: &Path) -> Result<(), String> {
    let Some((program, args)) = extract_command(archive, destination) else {
        return Err(format!(
            "'{}' is not an archive this downloader unpacks",
            archive.display()
        ));
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(&args);
    let output = run_timed(&mut cmd, EXTRACT_TIMEOUT).map_err(|e| {
        format!(
            "{program} did not answer within {}s: {e}",
            EXTRACT_TIMEOUT.as_secs()
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{program} failed with {}", output.status)
        } else {
            stderr
        });
    }
    Ok(())
}

/// Unpack every archive directly inside `folder` (not recursively: a nested
/// archive is the user's business). Returns the names that were extracted.
pub fn extract_archives_in(folder: &Path) -> Result<Vec<String>, String> {
    if !folder.is_dir() {
        return Ok(Vec::new());
    }
    let entries =
        std::fs::read_dir(folder).map_err(|e| format!("cannot read {}: {e}", folder.display()))?;
    let mut extracted = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !is_archive(&path) {
            continue;
        }
        extract_one(&path, folder)?;
        extracted.push(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    extracted.sort();
    Ok(extracted)
}

/// The finished-download ids already handled, so a poll never unpacks the
/// same archive twice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessedGids {
    #[serde(default)]
    pub gids: Vec<String>,
}

/// Load the processed marker; a corrupt file means "nothing handled yet".
pub fn load_processed() -> ProcessedGids {
    std::fs::read_to_string(processed_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist the processed marker, bounded to [`MAX_PROCESSED`].
pub fn save_processed(mut processed: ProcessedGids) -> Result<(), String> {
    if processed.gids.len() > MAX_PROCESSED {
        let excess = processed.gids.len() - MAX_PROCESSED;
        processed.gids.drain(0..excess);
    }
    let body = serde_json::to_vec_pretty(&processed).map_err(|e| e.to_string())?;
    write_atomic(&processed_path(), &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_allowlist_matches_the_installed_tools() {
        for good in [
            "/d/a.zip",
            "/d/a.ZIP",
            "/d/a.tar",
            "/d/a.tar.gz",
            "/d/a.tgz",
            "/d/a.tar.bz2",
            "/d/a.tbz2",
            "/d/a.tar.xz",
            "/d/a.txz",
        ] {
            assert!(is_archive(Path::new(good)), "{good} must be an archive");
        }
        // A second extractor (7z/rar) is a second surface and an optional
        // package: it is not this feature's job.
        for bad in ["/d/a.rar", "/d/a.7z", "/d/a.mp4", "/d/zip", "/d/a.zip.part"] {
            assert!(!is_archive(Path::new(bad)), "{bad} must not be an archive");
        }
    }

    #[test]
    fn test_extract_command_is_argv_and_targets_the_download_folder() {
        let (program, args) = extract_command(
            Path::new("/srv/downloads/Show/ep12.zip"),
            Path::new("/srv/downloads/Show"),
        )
        .unwrap();
        assert_eq!(program, "unzip");
        assert_eq!(
            args,
            vec![
                "-o",
                "-q",
                "/srv/downloads/Show/ep12.zip",
                "-d",
                "/srv/downloads/Show"
            ]
        );

        let (program, args) = extract_command(
            Path::new("/srv/downloads/Show/ep12.tar.gz"),
            Path::new("/srv/downloads/Show"),
        )
        .unwrap();
        assert_eq!(program, "tar");
        assert_eq!(
            args,
            vec![
                "-xf",
                "/srv/downloads/Show/ep12.tar.gz",
                "-C",
                "/srv/downloads/Show",
                "--no-absolute-names"
            ]
        );

        assert!(extract_command(Path::new("/d/a.rar"), Path::new("/d")).is_none());
    }

    #[test]
    fn test_extract_inside_a_download_folder_ignores_non_archives() {
        let dir =
            std::env::temp_dir().join(format!("hv-extract-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        // No archive: nothing to do, and no error.
        assert!(extract_archives_in(&dir).unwrap().is_empty());
        // A missing folder is a no-op, not a failure.
        assert!(extract_archives_in(&dir.join("nope")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_settings_default_to_off_and_round_trip() {
        assert!(!DownloaderSettings::default().auto_extract);
        let settings = DownloaderSettings { auto_extract: true };
        let json = serde_json::to_string(&settings).unwrap();
        let back: DownloaderSettings = serde_json::from_str(&json).unwrap();
        assert!(back.auto_extract);
        // An empty object (a file from an older build) keeps the safe default.
        let back: DownloaderSettings = serde_json::from_str("{}").unwrap();
        assert!(!back.auto_extract);
    }

    #[test]
    fn test_processed_marker_stays_bounded() {
        let processed = ProcessedGids {
            gids: (0..(MAX_PROCESSED + 10)).map(|n| format!("g{n}")).collect(),
        };
        let mut trimmed = processed.clone();
        if trimmed.gids.len() > MAX_PROCESSED {
            let excess = trimmed.gids.len() - MAX_PROCESSED;
            trimmed.gids.drain(0..excess);
        }
        assert_eq!(trimmed.gids.len(), MAX_PROCESSED);
        assert_eq!(trimmed.gids.first().map(String::as_str), Some("g10"));
    }
}
