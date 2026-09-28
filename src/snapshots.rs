use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;
use tracing::info;

/// Errors that can occur during snapshot operations.
#[derive(Error, Debug)]
pub enum SnapError {
    #[error("Snapshot command failed: {0}")]
    CommandFailed(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Pool not found: {0}")]
    PoolNotFound(String),
    #[error("Snapshot not found: {0}")]
    NotFound(String),
    #[error("Permission denied")]
    PermissionDenied,
}

/// A single filesystem snapshot (ZFS or Btrfs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub pool: String,
    pub created: String,
    pub used: u64,
    pub referenced: u64,
    pub properties: std::collections::HashMap<String, String>,
}

/// A dataset and its associated snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDataset {
    pub dataset: String,
    pub snapshots: Vec<Snapshot>,
}

/// Parameters for creating a new snapshot. The pool name comes from the URL
/// path (`handle_snapshot_create`'s `Path(pool)`), not this body — a `pool`
/// field here used to be required by Serde but never actually read, so the
/// real frontend (which correctly never sends a redundant `pool` in the
/// body) always failed with "missing field `pool`" before this was removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotRequest {
    pub name: String,
    pub dataset: Option<String>,
    pub recursive: bool,
}

/// Service for creating, listing, deleting, and rolling back ZFS/Btrfs snapshots.
pub struct SnapshotService;

impl SnapshotService {
    pub fn is_available() -> bool {
        Self::check_zfs().is_ok() || Self::check_btrfs().is_ok()
    }

    fn check_zfs() -> Result<(), SnapError> {
        let output = Command::new("sudo").arg("zfs").args(["--version"]).output();

        match output {
            Ok(out) => {
                if out.status.success() {
                    info!(
                        "ZFS available: {}",
                        String::from_utf8_lossy(&out.stdout).trim()
                    );
                    Ok(())
                } else {
                    Err(SnapError::CommandFailed("ZFS not available".to_string()))
                }
            }
            Err(_) => Err(SnapError::CommandFailed(
                "zfs command not found".to_string(),
            )),
        }
    }

    fn check_btrfs() -> Result<(), SnapError> {
        let output = Command::new("sudo").arg("btrfs").args(["--version"]).output();

        match output {
            Ok(out) => {
                if out.status.success() {
                    info!(
                        "Btrfs available: {}",
                        String::from_utf8_lossy(&out.stdout).trim()
                    );
                    Ok(())
                } else {
                    Err(SnapError::CommandFailed("Btrfs not available".to_string()))
                }
            }
            Err(_) => Err(SnapError::CommandFailed(
                "btrfs command not found".to_string(),
            )),
        }
    }

    pub fn list_snapshots(pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        if Self::check_zfs().is_ok() {
            Self::list_zfs_snapshots(pool)
        } else if Self::check_btrfs().is_ok() {
            Self::list_btrfs_snapshots(pool)
        } else {
            Err(SnapError::CommandFailed(
                "No snapshot support available".to_string(),
            ))
        }
    }

    fn list_zfs_snapshots(pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        let output = Command::new("sudo").arg("zfs")
            .args([
                "list",
                "-t",
                "snapshot",
                "-o",
                "name,used,referenced,creation",
                "-r",
                pool,
            ])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("dataset does not exist") {
                return Err(SnapError::PoolNotFound(pool.to_string()));
            }
            return Err(SnapError::CommandFailed(stderr.to_string()));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_zfs_snapshots(pool, &output)
    }

    /// Btrfs has no "pool name" of its own — a pool is just a mounted
    /// filesystem, and every `btrfs` subcommand needs a real path, not the
    /// label. Resolves the pool name (as reported by `PoolService::scan_pools`)
    /// to its actual mount point.
    fn resolve_btrfs_mount(pool: &str) -> Result<String, SnapError> {
        crate::pools::PoolService::btrfs_mount_point(pool)
            .ok_or_else(|| SnapError::PoolNotFound(format!("{} is not mounted", pool)))
    }

    fn list_btrfs_snapshots(pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        let mount_point = Self::resolve_btrfs_mount(pool)?;

        let output = Command::new("sudo").arg("btrfs")
            .args(["subvolume", "list", "-s", &mount_point])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_btrfs_snapshots(pool, &output)
    }

    pub fn create_snapshot(
        pool: &str,
        name: &str,
        dataset: Option<&str>,
        recursive: bool,
    ) -> Result<(), SnapError> {
        if Self::check_zfs().is_ok() {
            Self::create_zfs_snapshot(pool, name, dataset, recursive)
        } else if Self::check_btrfs().is_ok() {
            Self::create_btrfs_snapshot(pool, name, dataset)
        } else {
            Err(SnapError::CommandFailed(
                "No snapshot support available".to_string(),
            ))
        }
    }

    fn create_zfs_snapshot(
        pool: &str,
        name: &str,
        dataset: Option<&str>,
        recursive: bool,
    ) -> Result<(), SnapError> {
        let dataset_path = dataset.unwrap_or(pool);
        let snapshot_name = format!("{}@{}", dataset_path, name);

        let mut args = vec!["snapshot".to_string()];
        if recursive {
            args.push("-r".to_string());
        }
        args.push(snapshot_name.clone());

        let output = Command::new("sudo").arg("zfs")
            .args(&args)
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Created ZFS snapshot: {}", snapshot_name);
        Ok(())
    }

    fn create_btrfs_snapshot(
        pool: &str,
        name: &str,
        dataset: Option<&str>,
    ) -> Result<(), SnapError> {
        let mount_point = Self::resolve_btrfs_mount(pool)?;
        let source = dataset.unwrap_or(&mount_point);
        let snapshots_dir = format!("{}/.snapshots", mount_point);
        // sudo mkdir, not std::fs::create_dir_all — the pool's mount point
        // root is owned by root (it was formatted and mounted as root, via
        // sudo, in create_btrfs_advanced_pool), so the unprivileged
        // homevault process can't create a directory under it directly.
        let mkdir_output = Command::new("sudo")
            .arg("mkdir")
            .args(["-p", &snapshots_dir])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;
        if !mkdir_output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&mkdir_output.stderr).to_string(),
            ));
        }
        let snapshot_path = format!("{}/{}", snapshots_dir, name);

        let mut args = vec!["subvolume".to_string(), "snapshot".to_string()];
        args.push(source.to_string());
        args.push(snapshot_path.clone());

        let output = Command::new("sudo").arg("btrfs")
            .args(&args)
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Created Btrfs snapshot: {}", snapshot_path);
        Ok(())
    }

    pub fn delete_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        if Self::check_zfs().is_ok() {
            Self::delete_zfs_snapshot(pool, name)
        } else if Self::check_btrfs().is_ok() {
            Self::delete_btrfs_snapshot(pool, name)
        } else {
            Err(SnapError::CommandFailed(
                "No snapshot support available".to_string(),
            ))
        }
    }

    fn delete_zfs_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        let snapshot_name = if name.contains('@') {
            name.to_string()
        } else {
            format!("{}@{}", pool, name)
        };

        let output = Command::new("sudo").arg("zfs")
            .args(["destroy", &snapshot_name])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Deleted ZFS snapshot: {}", snapshot_name);
        Ok(())
    }

    fn delete_btrfs_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        let mount_point = Self::resolve_btrfs_mount(pool)?;
        let path = format!("{}/.snapshots/{}", mount_point, name);

        let output = Command::new("sudo").arg("btrfs")
            .args(["subvolume", "delete", &path])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Deleted Btrfs snapshot: {}", path);
        Ok(())
    }

    pub fn rollback_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        if Self::check_zfs().is_ok() {
            Self::rollback_zfs_snapshot(pool, name)
        } else if Self::check_btrfs().is_ok() {
            Self::rollback_btrfs_snapshot(pool, name)
        } else {
            Err(SnapError::CommandFailed(
                "No snapshot support available".to_string(),
            ))
        }
    }

    fn rollback_zfs_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        let snapshot_name = if name.contains('@') {
            name.to_string()
        } else {
            format!("{}@{}", pool, name)
        };

        let output = Command::new("sudo").arg("zfs")
            .args(["rollback", "-R", &snapshot_name])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Rolled back ZFS snapshot: {}", snapshot_name);
        Ok(())
    }

    fn rollback_btrfs_snapshot(pool: &str, _name: &str) -> Result<(), SnapError> {
        // Unlike `zfs rollback`, Btrfs has no single command that reverts a
        // *mounted, top-level* subvolume (subvolid=5, which is exactly how
        // `PoolService::create_pool` mounts every Btrfs pool) back to an
        // earlier snapshot in place — that requires either unmounting and
        // juggling default subvolumes, or a non-default `@`-subvolume mount
        // layout designed for it (as e.g. Snapper/openSUSE use). Doing this
        // naively (as an earlier version of this function attempted, via
        // `btrfs subvolume snapshot <snap> <pool>`) always fails: the
        // destination path is already an existing, mounted subvolume, and
        // `btrfs subvolume snapshot` refuses to write over one. Rather than
        // attempt a partial/unsafe rollback, fail clearly so the caller
        // knows to restore files from `<mount>/.snapshots/<name>` manually.
        let _ = Self::resolve_btrfs_mount(pool)?;
        Err(SnapError::CommandFailed(
            "In-place rollback isn't supported for Btrfs pools yet — browse and copy files back from the snapshot under <mount>/.snapshots/<name> instead".to_string(),
        ))
    }

    pub fn get_snapshot_info(pool: &str, name: &str) -> Result<Snapshot, SnapError> {
        if Self::check_zfs().is_ok() {
            Self::get_zfs_snapshot_info(pool, name)
        } else {
            Err(SnapError::CommandFailed(
                "Only ZFS supported for detailed info".to_string(),
            ))
        }
    }

    fn get_zfs_snapshot_info(pool: &str, name: &str) -> Result<Snapshot, SnapError> {
        let snapshot_name = if name.contains('@') {
            name.to_string()
        } else {
            format!("{}@{}", pool, name)
        };

        let output = Command::new("sudo").arg("zfs")
            .args([
                "get",
                "-H",
                "-o",
                "property,value",
                "-p",
                "all",
                &snapshot_name,
            ])
            .output()
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapError::NotFound(name.to_string()));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_zfs_snapshot_properties(pool, name, &output)
    }
}

fn parse_zfs_snapshots(pool: &str, output: &str) -> Result<Vec<Snapshot>, SnapError> {
    let mut snapshots = Vec::new();
    let lines = output.lines().skip(1);

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let full_name = parts[0];
            let used = parse_size(parts[1]).unwrap_or(0);
            let referenced = parse_size(parts[2]).unwrap_or(0);
            let created = parts[3].to_string();

            let snapshot_name = full_name.split('@').nth(1).unwrap_or(full_name);

            snapshots.push(Snapshot {
                name: snapshot_name.to_string(),
                pool: pool.to_string(),
                created,
                used,
                referenced,
                properties: std::collections::HashMap::new(),
            });
        }
    }

    Ok(snapshots)
}

fn parse_btrfs_snapshots(pool: &str, output: &str) -> Result<Vec<Snapshot>, SnapError> {
    let mut snapshots = Vec::new();

    // Real `btrfs subvolume list -s` output, e.g.:
    // "ID 256 gen 10 cgen 10 top level 5 otime 2026-07-14 19:57:51 path .snapshots/snap1"
    // Field positions aren't fixed-width, so look up each value by the label
    // token right before it rather than a hardcoded index.
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(path_idx) = parts.iter().position(|p| *p == "path") else {
            continue;
        };
        let Some(path) = parts.get(path_idx + 1) else {
            continue;
        };

        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string();

        let created = match parts.iter().position(|p| *p == "otime") {
            Some(i) => [parts.get(i + 1), parts.get(i + 2)]
                .into_iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            None => String::new(),
        };

        snapshots.push(Snapshot {
            name,
            pool: pool.to_string(),
            created,
            used: 0,
            referenced: 0,
            properties: std::collections::HashMap::new(),
        });
    }

    Ok(snapshots)
}

fn parse_zfs_snapshot_properties(
    pool: &str,
    name: &str,
    output: &str,
) -> Result<Snapshot, SnapError> {
    let mut properties = std::collections::HashMap::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            properties.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    Ok(Snapshot {
        name: name.to_string(),
        pool: pool.to_string(),
        created: properties.get("creation").cloned().unwrap_or_default(),
        used: properties
            .get("used")
            .and_then(|v| parse_size(v).ok())
            .unwrap_or(0),
        referenced: properties
            .get("referenced")
            .and_then(|v| parse_size(v).ok())
            .unwrap_or(0),
        properties,
    })
}

fn parse_size(s: &str) -> Result<u64, SnapError> {
    let s = s.trim();
    let multiplier: u64;
    let numeric: String;

    if s.ends_with('K') || s.ends_with("KB") {
        multiplier = 1024;
        numeric = s.trim_end_matches(['K', 'B']).to_string();
    } else if s.ends_with('M') || s.ends_with("MB") {
        multiplier = 1024 * 1024;
        numeric = s.trim_end_matches(['M', 'B']).to_string();
    } else if s.ends_with('G') || s.ends_with("GB") {
        multiplier = 1024 * 1024 * 1024;
        numeric = s.trim_end_matches(['G', 'B']).to_string();
    } else if s.ends_with('T') || s.ends_with("TB") {
        multiplier = 1024 * 1024 * 1024 * 1024;
        numeric = s.trim_end_matches(['T', 'B']).to_string();
    } else if s.ends_with('P') || s.ends_with("PB") {
        multiplier = 1024 * 1024 * 1024 * 1024 * 1024;
        numeric = s.trim_end_matches(['P', 'B']).to_string();
    } else {
        return s.parse().map_err(|_| SnapError::ParseError(s.to_string()));
    }

    let value: f64 = numeric
        .parse()
        .map_err(|_| SnapError::ParseError(s.to_string()))?;

    Ok((value * multiplier as f64) as u64)
}

impl Default for SnapshotService {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_size("invalid").is_err());
    }
}
