/* =============================================================================
 * File          : dedup.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Deduplication planning (tasklist P2-4). This module deliberately does NOT
 *   run a dedupe pass: the task requires a benchmark and a design decision
 *   first (docs/analysis/dedup-design.md). It validates paths and builds the
 *   argv a future executor/benchmark will use.
 *
 * Dependencies:
 *   - serde
 *
 * Usage:
 *   Planning/benchmark harness only; see docs/analysis/dedup-design.md.
 * =============================================================================
 */

//! Deduplication strategy and argv planning (no execution).

use serde::Serialize;

/// A dedupe pass is bounded to this many paths so one request cannot walk the
/// whole filesystem.
pub const MAX_DEDUP_PATHS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DedupStrategy {
    /// Btrfs extent dedupe via `duperemove`.
    BtrfsDuperemove,
    /// Pool-wide ZFS dedup (a property, not a per-file pass).
    ZfsPoolDedup,
}

/// Which strategy a filesystem type can use.
pub fn strategy_for(fstype: &str) -> Option<DedupStrategy> {
    match fstype.trim().to_ascii_lowercase().as_str() {
        "btrfs" => Some(DedupStrategy::BtrfsDuperemove),
        "zfs" => Some(DedupStrategy::ZfsPoolDedup),
        _ => None,
    }
}

/// Absolute path, no `..`, never inside `.snapshots` (deduping a snapshot
/// would rewrite the extents the snapshot shares with the live data).
pub fn validate_dedup_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() || !path.starts_with('/') {
        return Err("dedupe paths must be absolute".to_string());
    }
    if path.split('/').any(|part| part == "..") {
        return Err("dedupe paths must not contain '..'".to_string());
    }
    if path.contains("/.snapshots/") || path.ends_with("/.snapshots") {
        return Err("refusing to dedupe inside .snapshots".to_string());
    }
    Ok(path.to_string())
}

pub fn plan(paths: &[String]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("at least one path is required".to_string());
    }
    if paths.len() > MAX_DEDUP_PATHS {
        return Err(format!(
            "at most {MAX_DEDUP_PATHS} paths may be deduped in one pass"
        ));
    }
    paths.iter().map(|path| validate_dedup_path(path)).collect()
}

/// The argv for a future `duperemove` pass. `-r` is recursive, `-d` enables
/// dedupe, `-h` prints human-readable numbers. A persistent `--hashfile`
/// avoids re-hashing the same extents on every run.
pub fn duperemove_args(paths: &[String], hashfile: Option<&str>) -> Result<Vec<String>, String> {
    let paths = plan(paths)?;
    let mut args = vec![
        "sudo".to_string(),
        "duperemove".to_string(),
        "-r".to_string(),
        "-d".to_string(),
        "-h".to_string(),
    ];
    if let Some(hashfile) = hashfile {
        let hashfile = hashfile.trim();
        if hashfile.is_empty() || !hashfile.starts_with('/') || hashfile.contains("..") {
            return Err("the hashfile must be an absolute path without '..'".to_string());
        }
        args.push(format!("--hashfile={hashfile}"));
    }
    args.extend(paths);
    Ok(args)
}

/// ZFS dedup is a pool property; enabling it costs RAM for the dedup table
/// and cannot be undone by deleting files. The planning surface only states
/// the fact so an operator does not expect a per-directory pass.
pub fn zfs_dedup_note() -> &'static str {
    "ZFS dedup is a pool-wide property with a permanent RAM cost; it is not enabled by this module — benchmark it before `zfs set dedup=on`."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategies_are_explicit() {
        assert_eq!(strategy_for("btrfs"), Some(DedupStrategy::BtrfsDuperemove));
        assert_eq!(strategy_for("ZFS"), Some(DedupStrategy::ZfsPoolDedup));
        assert_eq!(strategy_for("ext4"), None);
    }

    #[test]
    fn paths_are_absolute_and_never_snapshots() {
        assert!(validate_dedup_path("/mnt/pools/photos").is_ok());
        assert!(validate_dedup_path("relative").is_err());
        assert!(validate_dedup_path("/mnt/../etc").is_err());
        assert!(validate_dedup_path("/mnt/pools/photos/.snapshots").is_err());
        assert!(plan(&[]).is_err());
        let too_many = vec!["/mnt/pools/photos".to_string(); MAX_DEDUP_PATHS + 1];
        assert!(plan(&too_many).is_err());
    }

    #[test]
    fn duperemove_argv_is_planned_but_not_run() {
        let paths = vec!["/mnt/pools/photos".to_string()];
        let args = duperemove_args(&paths, Some("/mnt/pools/.dedupe-hash")).unwrap();
        assert_eq!(args[0], "sudo");
        assert_eq!(args[1], "duperemove");
        assert!(args.contains(&"-r".to_string()));
        assert!(args.contains(&"-d".to_string()));
        assert!(args.contains(&"--hashfile=/mnt/pools/.dedupe-hash".to_string()));
        assert!(duperemove_args(&paths, Some("relative-hash")).is_err());
    }
}
