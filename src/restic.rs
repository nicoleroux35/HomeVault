use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum ResticError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Restic CLI not found")]
    NotFound,
    #[error("Repository not initialized: {0}")]
    NotInitialized(String),
    #[error("Restic command failed: {0}")]
    CommandFailed(String),
    #[error("JSON parse error: {0}")]
    ParseError(String),
    #[error("Snapshot error: {0}")]
    SnapError(#[from] crate::snapshots::SnapError),
    #[error("Backup not found: {0}")]
    BackupNotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticRepo {
    pub id: String,
    pub name: String,
    pub repository: String,
    pub password: String,
    pub retention_policy_id: Option<String>,
    pub source_paths: Vec<String>,
    pub exclude_patterns: Option<String>,
    pub tags: Option<String>,
    pub snapshot_source: String,
    pub snapshot_pool: Option<String>,
    pub snapshot_dataset: Option<String>,
    pub snapshot_create_before_backup: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticSnapshot {
    pub id: String,
    pub short_id: String,
    pub time: String,
    pub tree: String,
    pub paths: Vec<String>,
    pub hostname: String,
    pub program_version: String,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticBackupResult {
    pub snapshot_id: String,
    pub files_new: u64,
    pub files_changed: u64,
    pub files_unmodified: u64,
    pub data_added_bytes: u64,
    pub total_files_processed: u64,
    pub total_bytes: u64,
    pub duration_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticStats {
    pub total_size: u64,
    pub total_file_count: u64,
    pub snapshots_count: u64,
    pub total_blob_count: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticCheckResult {
    pub healthy: bool,
    pub message: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticProgress {
    pub percent: f64,
    pub message: String,
    pub current_files: u64,
    pub total_files: u64,
    pub bytes_done: u64,
    pub total_bytes: u64,
}

pub struct ResticService;

impl ResticService {
    fn restic_cmd() -> Result<Command, ResticError> {
        let which = Command::new("which").arg("restic").output();
        match which {
            Ok(out) if out.status.success() => Ok(Command::new("restic")),
            _ => Err(ResticError::NotFound),
        }
    }

    fn set_env(cmd: &mut Command, repo: &ResticRepo) {
        cmd.env("RESTIC_REPOSITORY", &repo.repository);
        cmd.env("RESTIC_PASSWORD", &repo.password);
    }

    pub fn is_available() -> bool {
        Command::new("restic")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn get_version() -> Result<String, ResticError> {
        let output = Command::new("restic")
            .arg("version")
            .output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(ResticError::NotFound)
        }
    }

    pub fn init_repo(repo: &ResticRepo) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("init");

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic repository initialized: {}", repo.repository);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already initialized") {
                info!("Restic repository already initialized: {}", repo.repository);
                return Ok(());
            }
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn create_backup(
        repo: &ResticRepo,
        paths: &[String],
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
    ) -> Result<ResticBackupResult, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("backup");
        cmd.arg("--json");

        if let Some(exclude) = exclude_patterns {
            for pattern in exclude.split('\n') {
                let p = pattern.trim();
                if !p.is_empty() {
                    cmd.arg("--exclude");
                    cmd.arg(p);
                }
            }
        }

        if let Some(t) = tags {
            for tag in t.split(',') {
                let tg = tag.trim();
                if !tg.is_empty() {
                    cmd.arg("--tag");
                    cmd.arg(tg);
                }
            }
        }

        for path in paths {
            cmd.arg(path);
        }

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResticError::CommandFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = Self::parse_backup_summary(&stdout)?;

        Ok(ResticBackupResult {
            snapshot_id: parsed
                .get("snapshot_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            files_new: parsed
                .get("files_new")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            files_changed: parsed
                .get("files_changed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            files_unmodified: parsed
                .get("files_unmodified")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            data_added_bytes: parsed
                .get("data_added")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            total_files_processed: parsed
                .get("total_files_processed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            total_bytes: parsed
                .get("total_bytes_processed")
                .or_else(|| parsed.get("total_bytes"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            duration_secs: parsed
                .get("total_duration")
                .and_then(|v| v.as_f64())
                .map(|d| d as u64)
                .unwrap_or(0),
        })
    }

    /// `restic backup --json` (0.16.x) emits **newline-delimited JSON**: a
    /// stream of `{"message_type":"status",...}` progress objects followed by
    /// one final `{"message_type":"summary",...}`. Parsing the whole blob as
    /// a single object fails ("trailing characters at line 2") — and because
    /// that parse happened *after* restic had already run, every manual and
    /// scheduled backup reported "Backup failed" even though the snapshot was
    /// created. Take the last non-`status` JSON object (the summary).
    fn parse_backup_summary(
        stdout: &str,
    ) -> Result<HashMap<String, serde_json::Value>, ResticError> {
        stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<HashMap<String, serde_json::Value>>(l).ok())
            .filter(|v| {
                v.get("message_type").and_then(|m| m.as_str()) != Some("status")
            })
            .next_back()
            .ok_or_else(|| {
                ResticError::ParseError(format!(
                    "no summary object in restic backup output: {stdout}"
                ))
            })
    }

    pub fn create_backup_with_snapshot(
        repo: &ResticRepo,
        pool: &str,
        dataset: &str,
        snapshot_name: &str,
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
    ) -> Result<ResticBackupResult, ResticError> {
        let snapshot_path = format!("{}@{}", dataset, snapshot_name);

        let snap_result = crate::snapshots::SnapshotService::create_snapshot(
            pool,
            snapshot_name,
            Some(dataset),
            false,
        );

        if let Err(e) = snap_result {
            return Err(ResticError::CommandFailed(format!(
                "Failed to create snapshot {}: {}",
                snapshot_path, e
            )));
        }

        info!("Created ZFS/Btrfs snapshot: {}", snapshot_path);

        let mount_point = format!("/tmp/restic-snap-{}", snapshot_name);
        let _ = std::fs::create_dir_all(&mount_point);

        let _mount_result = if crate::snapshots::SnapshotService::is_available() {
            let zfs_check = Command::new("zfs").arg("--version").output();
            if zfs_check.is_ok() && zfs_check.unwrap().status.success() {
                Command::new("mount")
                    .arg("-t")
                    .arg("zfs")
                    .arg(&snapshot_path)
                    .arg(&mount_point)
                    .output()
            } else {
                Command::new("mount")
                    .arg("--bind")
                    .arg(dataset)
                    .arg(&mount_point)
                    .output()
            }
        } else {
            Command::new("mount")
                .arg("--bind")
                .arg(dataset)
                .arg(&mount_point)
                .output()
        };

        let result = Self::create_backup(repo, std::slice::from_ref(&mount_point), exclude_patterns, tags);

        let _ = Command::new("umount").arg(&mount_point).output();
        let _ = std::fs::remove_dir(&mount_point);

        let _ = crate::snapshots::SnapshotService::delete_snapshot(pool, &snapshot_path);

        result
    }

    pub fn list_snapshots(
        repo: &ResticRepo,
    ) -> Result<Vec<ResticSnapshot>, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["snapshots", "--json"]);

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResticError::CommandFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let snapshots: Vec<ResticSnapshot> = serde_json::from_str(&stdout)
            .map_err(|e| ResticError::ParseError(e.to_string()))?;

        Ok(snapshots)
    }

    pub fn restore_snapshot(
        repo: &ResticRepo,
        snapshot_id: &str,
        target: &str,
        exclude_patterns: Option<&str>,
        include_paths: Option<&[String]>,
    ) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["restore", snapshot_id, "--target", target]);

        if let Some(exclude) = exclude_patterns {
            for pattern in exclude.split('\n') {
                let p = pattern.trim();
                if !p.is_empty() {
                    cmd.arg("--exclude");
                    cmd.arg(p);
                }
            }
        }

        if let Some(include) = include_paths {
            for path in include {
                cmd.arg("--include");
                cmd.arg(path);
            }
        }

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic restore completed: {} -> {}", snapshot_id, target);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn check_repo(
        repo: &ResticRepo,
        read_data: bool,
    ) -> Result<ResticCheckResult, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("check");

        if read_data {
            cmd.arg("--read-data");
        }

        let output = cmd.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let all_output = format!("{}\n{}", stdout, stderr);

        let mut errors = Vec::new();
        for line in all_output.lines() {
            if line.contains("error") || line.contains("Error") {
                errors.push(line.to_string());
            }
        }

        Ok(ResticCheckResult {
            healthy: output.status.success() && errors.is_empty(),
            message: if output.status.success() {
                "Repository is healthy".to_string()
            } else {
                "Repository check found issues".to_string()
            },
            errors,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn forget_snapshots(
        repo: &ResticRepo,
        keep_last: Option<u64>,
        keep_hourly: Option<u64>,
        keep_daily: Option<u64>,
        keep_weekly: Option<u64>,
        keep_monthly: Option<u64>,
        keep_yearly: Option<u64>,
        keep_within: Option<String>,
        prune: bool,
    ) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("forget");

        if let Some(n) = keep_last {
            cmd.args(["--keep-last", &n.to_string()]);
        }
        if let Some(n) = keep_hourly {
            cmd.args(["--keep-hourly", &n.to_string()]);
        }
        if let Some(n) = keep_daily {
            cmd.args(["--keep-daily", &n.to_string()]);
        }
        if let Some(n) = keep_weekly {
            cmd.args(["--keep-weekly", &n.to_string()]);
        }
        if let Some(n) = keep_monthly {
            cmd.args(["--keep-monthly", &n.to_string()]);
        }
        if let Some(n) = keep_yearly {
            cmd.args(["--keep-yearly", &n.to_string()]);
        }
        if let Some(ref w) = keep_within {
            cmd.args(["--keep-within", w]);
        }

        if prune {
            cmd.arg("--prune");
        }

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic forget completed for repo: {}", repo.name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn prune_repo(repo: &ResticRepo) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("prune");

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic prune completed for repo: {}", repo.name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn get_stats(repo: &ResticRepo) -> Result<ResticStats, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["stats", "--json"]);

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResticError::CommandFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let raw: HashMap<String, serde_json::Value> = serde_json::from_str(&stdout)
            .map_err(|e| ResticError::ParseError(e.to_string()))?;

        Ok(ResticStats {
            total_size: raw.get("total_size").and_then(|v| v.as_u64()).unwrap_or(0),
            total_file_count: raw
                .get("total_file_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            snapshots_count: raw
                .get("snapshots_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            total_blob_count: raw
                .get("total_blob_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            compressed_size: raw
                .get("compressed_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        })
    }

    pub fn unlock_repo(repo: &ResticRepo) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("unlock");

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic repo unlocked: {}", repo.name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn diff_snapshots(
        repo: &ResticRepo,
        snapshot_a: &str,
        snapshot_b: &str,
    ) -> Result<String, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["diff", snapshot_a, snapshot_b]);

        let output = cmd.output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn find_in_snapshots(
        repo: &ResticRepo,
        pattern: &str,
        snapshot_id: Option<&str>,
    ) -> Result<String, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["find", pattern]);

        if let Some(snap) = snapshot_id {
            cmd.args(["--snapshot", snap]);
        }

        let output = cmd.output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn delete_snapshot(
        repo: &ResticRepo,
        snapshot_id: &str,
    ) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["forget", snapshot_id]);

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic snapshot deleted: {}", snapshot_id);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    //! Pure parsing tests for `ResticService`.
    //!
    //! Fixtures are real output shapes from `restic` 0.16.x. The important
    //! one is `parse_backup_summary`: `restic backup --json` streams
    //! newline-delimited JSON (status lines + a final summary), which the
    //! old whole-blob `serde_json::from_str` choked on — so every backup
    //! reported failure even though the snapshot was created.
    //!
    //! Run: `cargo test restic::tests`

    use super::*;

    const BACKUP_JSON: &str = r#"{"message_type":"status","percent_done":0,"total_files":1,"total_bytes":6}
{"message_type":"status","percent_done":0.5,"files_done":0,"bytes_done":0,"total_files":1,"total_bytes":6}
{"message_type":"status","percent_done":1,"files_done":1,"bytes_done":6,"total_files":1,"total_bytes":6}
{"message_type":"summary","files_new":1,"files_changed":0,"files_unmodified":0,"dirs_new":1,"dirs_changed":0,"dirs_unmodified":0,"data_blobs":1,"tree_blobs":1,"data_added":419,"total_files_processed":1,"total_bytes_processed":6,"total_duration":0.012345,"snapshot_id":"da0f81d6ff3be9013fb2646c8f779504c12e2fb2769b2d5923fd3c45f9db1a6e"}"#;

    #[test]
    fn test_parse_backup_summary_picks_the_summary_line() {
        let s = ResticService::parse_backup_summary(BACKUP_JSON).expect("must parse");
        assert_eq!(s.get("message_type").and_then(|v| v.as_str()), Some("summary"));
        assert_eq!(s.get("files_new").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(s.get("data_added").and_then(|v| v.as_u64()), Some(419));
        assert_eq!(s.get("total_bytes_processed").and_then(|v| v.as_u64()), Some(6));
        assert!(s.get("snapshot_id").and_then(|v| v.as_str()).unwrap().starts_with("da0f81d6"));
    }

    #[test]
    fn test_parse_backup_summary_rejects_empty_output() {
        assert!(ResticService::parse_backup_summary("").is_err());
        assert!(ResticService::parse_backup_summary("   \n\n").is_err());
    }

    #[test]
    fn test_parse_backup_summary_tolerates_only_status_lines() {
        // A backup with no summary object (interrupted) — must not panic,
        // must not silently pick a status line.
        let only_status = "{\"message_type\":\"status\",\"percent_done\":0.3}\n";
        assert!(ResticService::parse_backup_summary(only_status).is_err());
    }

    #[test]
    fn test_parse_backup_summary_single_line_still_works() {
        // Older/quieter restic could emit just the summary.
        let one = r#"{"message_type":"summary","files_new":2,"data_added":10,"total_bytes_processed":20,"total_duration":1.5,"snapshot_id":"abc123"}"#;
        let s = ResticService::parse_backup_summary(one).unwrap();
        assert_eq!(s.get("files_new").and_then(|v| v.as_u64()), Some(2));
    }

    #[test]
    fn test_backup_result_mapping_from_summary() {
        // End-to-end of the mapping in create_backup (minus the Command).
        let s = ResticService::parse_backup_summary(BACKUP_JSON).unwrap();
        let result = ResticBackupResult {
            snapshot_id: s.get("snapshot_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            files_new: s.get("files_new").and_then(|v| v.as_u64()).unwrap_or(0),
            files_changed: s.get("files_changed").and_then(|v| v.as_u64()).unwrap_or(0),
            files_unmodified: s.get("files_unmodified").and_then(|v| v.as_u64()).unwrap_or(0),
            data_added_bytes: s.get("data_added").and_then(|v| v.as_u64()).unwrap_or(0),
            total_files_processed: s.get("total_files_processed").and_then(|v| v.as_u64()).unwrap_or(0),
            total_bytes: s.get("total_bytes_processed").or_else(|| s.get("total_bytes")).and_then(|v| v.as_u64()).unwrap_or(0),
            duration_secs: s.get("total_duration").and_then(|v| v.as_f64()).map(|d| d as u64).unwrap_or(0),
        };
        assert_eq!(result.files_new, 1);
        assert_eq!(result.data_added_bytes, 419);
        assert_eq!(result.total_files_processed, 1);
        assert_eq!(result.total_bytes, 6);
        assert_eq!(result.duration_secs, 0); // 0.0123s truncates to 0
        assert!(!result.snapshot_id.is_empty());
    }

    // -- ResticSnapshot deserialization (restic snapshots --json) --------

    #[test]
    fn test_snapshot_list_deserializes_real_json() {
        let raw = r#"[
          {"time":"2026-09-06T14:53:02.752523986+02:00","tree":"c87bcc11","paths":["/srv/data"],
           "hostname":"nas","username":"root","program_version":"restic 0.16.4",
           "tags":["daily"],"id":"da0f81d6ff3be9013fb2646c8f779504","short_id":"da0f81d6"},
          {"time":"2026-09-05T14:53:02Z","tree":"aa11","paths":["/srv/data","/etc"],
           "hostname":"nas","program_version":"restic 0.16.4",
           "id":"bb22cc33dd44","short_id":"bb22cc33"}
        ]"#;
        let snaps: Vec<ResticSnapshot> = serde_json::from_str(raw).expect("must parse");
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].short_id, "da0f81d6");
        assert_eq!(snaps[0].tags.as_deref(), Some(&["daily".to_string()][..]));
        assert_eq!(snaps[1].paths, vec!["/srv/data", "/etc"]);
        // second snapshot has no `tags` key at all → None, not an error
        assert!(snaps[1].tags.is_none());
    }

    #[test]
    fn test_empty_snapshot_list_parses() {
        let snaps: Vec<ResticSnapshot> = serde_json::from_str("[]").unwrap();
        assert!(snaps.is_empty());
    }

    // -- serialization contracts (frontend expects these field names) ----

    #[test]
    fn test_result_structs_serialize_with_snake_case_keys() {
        let r = ResticBackupResult {
            snapshot_id: "x".into(),
            files_new: 1,
            files_changed: 2,
            files_unmodified: 3,
            data_added_bytes: 4,
            total_files_processed: 5,
            total_bytes: 6,
            duration_secs: 7,
        };
        let v = serde_json::to_value(&r).unwrap();
        for k in ["snapshot_id", "files_new", "data_added_bytes", "total_files_processed", "duration_secs"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }

        let stats = ResticStats {
            total_size: 1, total_file_count: 2, snapshots_count: 3,
            total_blob_count: 4, compressed_size: 5,
        };
        let sv = serde_json::to_value(&stats).unwrap();
        assert_eq!(sv["snapshots_count"], 3);
    }
}
