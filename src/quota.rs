/* =============================================================================
 * File          : quota.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-user home-directory disk quotas applied through the filesystem's
 *   setquota tooling on the volume hosting the user's home folder.
 *
 * Dependencies:
 *   - thiserror (>= 2)
 *
 * Usage:
 *   Called by the user-home handlers when creating users or changing the
 *   home-folder size limit.
 * =============================================================================
 */

//! Per-user home-directory disk quotas (setquota on the hosting volume).

use std::process::Command;
use thiserror::Error;

/// Errors from applying a per-user home-directory quota via `setquota`.
#[derive(Error, Debug)]
pub enum QuotaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// Per-user home-directory disk-space quota helpers (`setquota -u`).
///
/// The old per-group filesystem-quota feature — `QuotaService` CRUD methods,
/// the `/api/quotas*` routes, `storage::quota_repo`, and the Settings
/// "Disk Quota" card — was removed 2026-09-09: quotas are managed per user on
/// the Users page and (for transfer metering) per user/group on the Users and
/// Groups pages. Only the home-dir helpers `handlers/user_homes.rs` needs
/// remain here.
pub struct QuotaService;

impl QuotaService {
    /// Convert a byte count into the 1-KiB block count that `setquota` expects.
    /// Rounds down; negative inputs clamp to 0.
    pub fn bytes_to_quota_kb(bytes: i64) -> i64 {
        bytes.max(0) / 1024
    }

    /// Apply a per-user disk quota on `filesystem` via `setquota -u`.
    ///
    /// Sets the soft and hard block limits both to `hard_bytes` (inode limits
    /// left at 0 = unlimited). A `hard_bytes` of 0 removes the quota.
    ///
    /// Best-effort: returns `Err` when the `setquota` binary is missing or the
    /// target filesystem was not mounted with `usrquota`, so the caller can tell
    /// the admin the limit was recorded but not actively enforced. Bare command
    /// (no `sudo`) to match the rest of this module; the process runs as root.
    pub fn set_user_home_quota(
        system_username: &str,
        filesystem: &str,
        hard_bytes: i64,
    ) -> Result<(), QuotaError> {
        let kb = Self::bytes_to_quota_kb(hard_bytes);
        let output = Command::new("setquota")
            .args([
                "-u",
                system_username,
                &kb.to_string(),
                &kb.to_string(),
                "0",
                "0",
                filesystem,
            ])
            .output()
            .map_err(QuotaError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(QuotaError::CommandFailed(stderr.trim().to_string()));
        }

        tracing::info!(
            "Applied home-dir quota for user {} on {}: {}KB",
            system_username,
            filesystem,
            kb
        );
        Ok(())
    }

    pub fn format_size_kb(kb: i64) -> String {
        if kb == 0 {
            return "Unlimited".to_string();
        }
        if kb < 1024 {
            return format!("{} KB", kb);
        }
        if kb < 1024 * 1024 {
            return format!("{:.1} MB", kb as f64 / 1024.0);
        }
        format!("{:.2} GB", kb as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_quota_kb_rounds_down() {
        assert_eq!(QuotaService::bytes_to_quota_kb(0), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(1), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(1023), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(1024), 1);
        assert_eq!(QuotaService::bytes_to_quota_kb(1536), 1);
        assert_eq!(
            QuotaService::bytes_to_quota_kb(10 * 1024 * 1024 * 1024),
            10 * 1024 * 1024
        );
    }

    #[test]
    fn test_bytes_to_quota_kb_clamps_negative() {
        assert_eq!(QuotaService::bytes_to_quota_kb(-1), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(-99_999), 0);
    }

    #[test]
    fn test_format_size_kb_zero_is_unlimited() {
        assert_eq!(QuotaService::format_size_kb(0), "Unlimited");
    }

    #[test]
    fn test_format_size_kb_below_one_mb() {
        assert_eq!(QuotaService::format_size_kb(1), "1 KB");
        assert_eq!(QuotaService::format_size_kb(512), "512 KB");
        assert_eq!(QuotaService::format_size_kb(1023), "1023 KB");
    }

    #[test]
    fn test_format_size_kb_in_mb_range() {
        assert_eq!(QuotaService::format_size_kb(1024), "1.0 MB");
        assert_eq!(QuotaService::format_size_kb(1536), "1.5 MB");
        assert_eq!(QuotaService::format_size_kb(1048575), "1024.0 MB");
    }

    #[test]
    fn test_format_size_kb_in_gb_range() {
        assert_eq!(QuotaService::format_size_kb(1048576), "1.00 GB");
        assert_eq!(QuotaService::format_size_kb(2097152), "2.00 GB");
        assert_eq!(QuotaService::format_size_kb(1572864), "1.50 GB");
    }

    #[test]
    fn test_quota_error_display() {
        let err = QuotaError::CommandFailed("setquota: not found".to_string());
        assert!(err.to_string().contains("setquota"));
    }
}
