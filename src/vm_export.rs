/* =============================================================================
 * File          : vm_export.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Virtual-machine portability: export a machine (or one of its snapshots)
 *   to a single archive with `incus export`, list what HomeVault has exported,
 *   import an archive back as a new machine with `incus import`, and delete an
 *   archive.
 *
 *   This is DSM's "OVA export/import", implemented on the native Incus
 *   primitive instead of a second imaging subsystem: the archive contains the
 *   instance configuration and its root filesystem, and `incus import` accepts
 *   it on the same or another HomeVault. The files live in
 *   `<DATA_DIRECTORY>/vm/exports`, one folder HomeVault owns, so a request can
 *   never name a path anywhere else.
 *
 *   Every rule that decides *whether* something may run is pure and
 *   unit-tested (archive name, target path, argv); only `export`, `import`,
 *   `list` and `delete` touch the host, through bounded `sudo incus` calls.
 *
 * Dependencies:
 *   - serde (>= 1.0), crate::utils::process, crate::config::paths
 *
 * Usage:
 *   handlers::vm exposes /api/vm/exports; see docs/guides/vm.md.
 * =============================================================================
 */

//! Virtual-machine export/import (see docs/guides/vm.md).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Deadline for an export. A 100 GB machine writes its whole disk: this is a
/// ceiling, not an expectation, and it exists so a wedged `incus` cannot pin
/// the blocking worker for the life of the service.
pub const EXPORT_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);

/// Deadline for an import (unpacking + creating the instance).
pub const IMPORT_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);

/// Deadline for the capability probe (`which incus`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest accepted archive file name.
pub const MAX_NAME_LEN: usize = 128;

/// How many archives one listing returns (newest first).
pub const MAX_LISTED: usize = 200;

/// One exported archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmExport {
    /// File name inside the export folder (never a path).
    pub file: String,
    pub size_bytes: u64,
    /// RFC 3339 mtime, best effort.
    pub created_at: Option<String>,
}

/// `incus export` writes `<instance>[-<snapshot>]-<stamp>.tar[.gz]`.
///
/// The name is derived here, from values already validated by the VM handlers,
/// so the archive a request creates can never land outside the export folder.
pub fn archive_name(
    instance: &str,
    snapshot: Option<&str>,
    stamp: &str,
    compressed: bool,
) -> String {
    let mut name = instance.trim().to_string();
    if let Some(snapshot) = snapshot.map(str::trim).filter(|s| !s.is_empty()) {
        name.push('-');
        name.push_str(snapshot);
    }
    name.push('-');
    name.push_str(stamp);
    if compressed {
        name.push_str(".tar.gz");
    } else {
        name.push_str(".tar");
    }
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .take(MAX_NAME_LEN)
        .collect()
}

/// An archive name must stay one file in the export folder: no separator, no
/// traversal, no leading dot, and one of the two extensions we produce.
pub fn validate_archive_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err("Invalid archive name".to_string());
    }
    if name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(char::is_control)
    {
        return Err("Invalid archive name".to_string());
    }
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".tar") && !lower.ends_with(".tar.gz") {
        return Err("A machine archive must end in .tar or .tar.gz".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("Invalid archive name".to_string());
    }
    Ok(name.to_string())
}

/// The absolute path of one archive, confined to `folder`.
pub fn archive_path(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let name = validate_archive_name(name)?;
    Ok(folder.join(name))
}

/// `incus export [--compressed] <instance> [<snapshot>] <target>`.
pub fn export_argv(
    instance: &str,
    snapshot: Option<&str>,
    target: &Path,
    compressed: bool,
) -> Vec<String> {
    let mut argv = vec!["incus".to_string(), "export".to_string()];
    if compressed {
        argv.push("--compressed".to_string());
    }
    argv.push(instance.trim().to_string());
    if let Some(snapshot) = snapshot.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push(snapshot.to_string());
    }
    argv.push(target.to_string_lossy().into_owned());
    argv
}

/// `incus import <source> [<name>]`.
pub fn import_argv(source: &Path, name: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "incus".to_string(),
        "import".to_string(),
        source.to_string_lossy().into_owned(),
    ];
    if let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push(name.to_string());
    }
    argv
}

/// Whether the `incus` CLI is installed (`503` semantics when it is not).
pub fn cli_available() -> bool {
    run_timed(
        std::process::Command::new("which").arg("incus"),
        PROBE_TIMEOUT,
    )
    .map(|out| out.status.success())
    .unwrap_or(false)
}

fn run_sudo(args: &[String], deadline: Duration) -> Result<String, String> {
    let Some((program, rest)) = args.split_first() else {
        return Err("empty command".to_string());
    };
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(program).args(rest);
    let output = run_timed(&mut cmd, deadline).map_err(|e| {
        format!(
            "{program} did not answer within {}s: {e}",
            deadline.as_secs()
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
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The export folder, created on first use.
pub fn ensure_dir() -> Result<PathBuf, String> {
    let dir = crate::config::paths::vm_export_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Every archive in the folder, newest first. A file that is not one of our
/// archive names is ignored rather than listed as a broken entry.
pub fn read_entries(dir: &Path) -> Vec<VmExport> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<(std::time::SystemTime, VmExport)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if validate_archive_name(name).is_err() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().ok();
        let created_at = modified.and_then(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|duration| chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0))
                .map(|dt| dt.to_rfc3339())
        });
        list.push((
            modified.unwrap_or(std::time::UNIX_EPOCH),
            VmExport {
                file: name.to_string(),
                size_bytes: metadata.len(),
                created_at,
            },
        ));
    }
    list.sort_by(|a, b| b.0.cmp(&a.0));
    list.into_iter().take(MAX_LISTED).map(|(_, e)| e).collect()
}

/// Export one machine (or snapshot) into the export folder. Returns the file
/// entry that was written.
pub fn export(
    instance: &str,
    snapshot: Option<&str>,
    compressed: bool,
) -> Result<VmExport, String> {
    let dir = ensure_dir()?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let name = archive_name(instance, snapshot, &stamp, compressed);
    let target = archive_path(&dir, &name)?;
    run_sudo(
        &export_argv(instance, snapshot, &target, compressed),
        EXPORT_TIMEOUT,
    )?;
    let size_bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    if size_bytes == 0 {
        return Err(format!(
            "incus reported success but {} is empty",
            target.display()
        ));
    }
    Ok(VmExport {
        file: name,
        size_bytes,
        created_at: Some(chrono::Utc::now().to_rfc3339()),
    })
}

/// Import an archive as a (new) machine.
pub fn import(file: &str, name: Option<&str>) -> Result<String, String> {
    let dir = ensure_dir()?;
    let source = archive_path(&dir, file)?;
    if !source.is_file() {
        return Err(format!("No archive named '{file}'"));
    }
    run_sudo(&import_argv(&source, name), IMPORT_TIMEOUT)?;
    Ok(source.to_string_lossy().into_owned())
}

/// Delete one archive. The name is re-validated, so a traversal never reaches
/// the filesystem.
pub fn delete(file: &str) -> Result<(), String> {
    let dir = ensure_dir()?;
    let path = archive_path(&dir, file)?;
    std::fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_name_is_derived_and_safe() {
        assert_eq!(
            archive_name("web-server", None, "20261109-120000", true),
            "web-server-20261109-120000.tar.gz"
        );
        assert_eq!(
            archive_name(
                "web-server",
                Some("before-upgrade"),
                "20261109-120000",
                false
            ),
            "web-server-before-upgrade-20261109-120000.tar"
        );
        // A character the name may not carry becomes a dash, so the file can
        // never leave the folder.
        assert_eq!(
            archive_name("weird/name", None, "x", false),
            "weird-name-x.tar"
        );
        assert!(archive_name("x", None, "t", true).len() <= MAX_NAME_LEN);
    }

    #[test]
    fn test_archive_name_validation() {
        assert_eq!(
            validate_archive_name(" web-1.tar.gz ").unwrap(),
            "web-1.tar.gz"
        );
        for bad in [
            "",
            "../etc/passwd",
            "a/b.tar",
            "/etc/passwd.tar",
            ".hidden.tar",
            "a.tar.gz.exe",
            "a.zip",
            "a.tar\0",
        ] {
            assert!(
                validate_archive_name(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn test_archive_path_stays_inside_the_folder() {
        let folder = Path::new("/srv/homevault/vm/exports");
        assert_eq!(
            archive_path(folder, "web.tar.gz").unwrap(),
            folder.join("web.tar.gz")
        );
        assert!(archive_path(folder, "../web.tar.gz").is_err());
    }

    #[test]
    fn test_argv_shapes() {
        let target = Path::new("/srv/homevault/vm/exports/web.tar.gz");
        assert_eq!(
            export_argv("web", None, target, true),
            vec![
                "incus",
                "export",
                "--compressed",
                "web",
                "/srv/homevault/vm/exports/web.tar.gz"
            ]
        );
        assert_eq!(
            export_argv("web", Some("snap0"), target, false),
            vec![
                "incus",
                "export",
                "web",
                "snap0",
                "/srv/homevault/vm/exports/web.tar.gz"
            ]
        );
        assert_eq!(
            import_argv(Path::new("/srv/homevault/vm/exports/web.tar.gz"), None),
            vec!["incus", "import", "/srv/homevault/vm/exports/web.tar.gz"]
        );
        assert_eq!(
            import_argv(
                Path::new("/srv/homevault/vm/exports/web.tar.gz"),
                Some("copy")
            ),
            vec![
                "incus",
                "import",
                "/srv/homevault/vm/exports/web.tar.gz",
                "copy"
            ]
        );
    }

    #[test]
    fn test_read_entries_ignores_foreign_files() {
        let dir =
            std::env::temp_dir().join(format!("hv-vm-exports-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("web-20261109.tar.gz"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"n").unwrap();
        std::fs::write(dir.join(".hidden.tar"), b"h").unwrap();
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file, "web-20261109.tar.gz");
        assert_eq!(entries[0].size_bytes, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
