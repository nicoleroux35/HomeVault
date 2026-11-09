/* =============================================================================
 * File          : zfs_volumes.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Thin/thick ZFS zvol management (tasklist P2-3): create a sparse volume
 *   with `refreservation=none`, list volumes with their usage and flag the
 *   ones nearing saturation. LVM thin volumes are not covered yet.
 *
 * Dependencies:
 *   - tokio, crate::utils::process
 *
 * Usage:
 *   `src/handlers/zfs_volumes.rs`; see docs/guides/storage.md.
 * =============================================================================
 */

//! ZFS zvol thin provisioning.

use std::time::Duration;

use serde::Serialize;

use crate::utils::process::run_timed_async_result;

const ZFS_QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const ZFS_WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// A volume at or above this percentage of its nominal size is flagged.
pub const SATURATION_PERCENT: u64 = 90;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ZfsVolume {
    pub name: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub referenced_bytes: u64,
    pub available_bytes: u64,
    /// `true` when the used space is at least `SATURATION_PERCENT` of the
    /// nominal size: a thin volume can still be written to, but the pool is
    /// heading for a surprise.
    pub saturated: bool,
}

/// Volume names are dataset paths: safe components, no `@`, no `..`.
pub fn validate_volume_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err("a volume name is required (max 200 characters)".to_string());
    }
    if name.contains('@') || name.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err("the volume name must be a dataset path without '@' or '..'".to_string());
    }
    if name.split('/').any(|part| {
        !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }) {
        return Err(
            "each volume path component may only use letters, digits, '_' and '-'".to_string(),
        );
    }
    Ok(name.to_string())
}

pub fn validate_volume_size(size_bytes: u64) -> Result<(), String> {
    const MIN: u64 = 1024 * 1024;
    const MAX: u64 = 1024u64.pow(4);
    if !(MIN..=MAX).contains(&size_bytes) {
        return Err(format!(
            "the volume size must be between {MIN} and {MAX} bytes"
        ));
    }
    Ok(())
}

/// `zfs create` argv. A thin volume is sparse (`-s`) and has no refreservation;
/// a thick one keeps ZFS's default reservation.
pub fn create_args(name: &str, size_bytes: u64, thin: bool) -> Result<Vec<String>, String> {
    let name = validate_volume_name(name)?;
    validate_volume_size(size_bytes)?;
    let mut args = vec!["sudo".to_string(), "zfs".to_string(), "create".to_string()];
    if thin {
        args.push("-s".to_string());
    }
    args.push("-V".to_string());
    args.push(size_bytes.to_string());
    args.push("-o".to_string());
    args.push("compression=lz4".to_string());
    if thin {
        args.push("-o".to_string());
        args.push("refreservation=none".to_string());
    }
    args.push(name);
    Ok(args)
}

pub fn destroy_args(name: &str) -> Result<Vec<String>, String> {
    let name = validate_volume_name(name)?;
    Ok(vec![
        "sudo".to_string(),
        "zfs".to_string(),
        "destroy".to_string(),
        name,
    ])
}

/// Parse `zfs list -H -p -t volume -o name,volsize,used,referenced,available`.
pub fn parse_volumes(output: &str) -> Vec<ZfsVolume> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 5 {
                return None;
            }
            let size_bytes = fields[1].trim().parse().ok()?;
            let used_bytes = fields[2].trim().parse().ok()?;
            let referenced_bytes = fields[3].trim().parse().ok()?;
            let available_bytes = fields[4].trim().parse().ok()?;
            Some(ZfsVolume {
                name: fields[0].trim().to_string(),
                size_bytes,
                used_bytes,
                referenced_bytes,
                available_bytes,
                saturated: is_saturated(used_bytes, size_bytes),
            })
        })
        .collect()
}

/// A volume is saturated at 90 % of its nominal size (or when it has no
/// declared size).
pub fn is_saturated(used: u64, size: u64) -> bool {
    size > 0 && used.saturating_mul(100) >= size.saturating_mul(SATURATION_PERCENT)
}

/// `true` when `zfs` is usable on this host.
pub fn zfs_available_at(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

pub async fn list_volumes() -> Result<Vec<ZfsVolume>, String> {
    let mut cmd = tokio::process::Command::new("zfs");
    cmd.args([
        "list",
        "-H",
        "-p",
        "-t",
        "volume",
        "-o",
        "name,volsize,used,referenced,available",
    ]);
    let output = run_timed_async_result(&mut cmd, ZFS_QUERY_TIMEOUT)
        .await
        .map_err(|e| format!("could not run zfs list: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "zfs list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_volumes(&String::from_utf8_lossy(&output.stdout)))
}

pub async fn create_volume(name: &str, size_bytes: u64, thin: bool) -> Result<(), String> {
    let argv = create_args(name, size_bytes, thin)?;
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    let output = run_timed_async_result(&mut cmd, ZFS_WRITE_TIMEOUT)
        .await
        .map_err(|e| format!("could not run zfs create: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "zfs create failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

pub async fn destroy_volume(name: &str) -> Result<(), String> {
    let argv = destroy_args(name)?;
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    let output = run_timed_async_result(&mut cmd, ZFS_WRITE_TIMEOUT)
        .await
        .map_err(|e| format!("could not run zfs destroy: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "zfs destroy failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_names_and_sizes_are_a_closed_set() {
        assert_eq!(
            validate_volume_name("tank/backups").unwrap(),
            "tank/backups"
        );
        assert!(validate_volume_name("../etc").is_err());
        assert!(validate_volume_name("tank/a b").is_err());
        assert!(validate_volume_name("tank/snap@now").is_err());
        assert!(validate_volume_size(1024 * 1024).is_ok());
        assert!(validate_volume_size(1024 * 1024 - 1).is_err());
    }

    #[test]
    fn thin_and_thick_volumes_get_different_argv() {
        let thin = create_args("tank/data", 10 * 1024 * 1024 * 1024, true).unwrap();
        assert!(thin.contains(&"-s".to_string()));
        assert!(thin.contains(&"refreservation=none".to_string()));
        let thick = create_args("tank/data", 10 * 1024 * 1024 * 1024, false).unwrap();
        assert!(!thick.contains(&"-s".to_string()));
        assert!(!thick.contains(&"refreservation=none".to_string()));
        assert_eq!(
            destroy_args("tank/data").unwrap(),
            vec!["sudo", "zfs", "destroy", "tank/data"]
        );
    }

    #[test]
    fn volumes_are_parsed_and_saturation_is_flagged() {
        let output = "\
tank/backups\t10737418240\t9663676416\t9663676410\t1073741824
tank/cold\t10737418240\t1024\t1024\t1073741824
";
        let volumes = parse_volumes(output);
        assert_eq!(volumes.len(), 2);
        assert!(volumes[0].saturated);
        assert!(!volumes[1].saturated);
        assert!(is_saturated(90, 100));
        assert!(!is_saturated(89, 100));
        assert!(!is_saturated(1, 0));
    }
}
