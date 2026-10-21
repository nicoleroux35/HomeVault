/* =============================================================================
 * File          : models.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Rescue toolkit types: the error enum, per-tool option structs and
 *   result models shared by the service and the handlers.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2)
 *
 * Usage:
 *   Imported by rescue/service.rs and handlers/rescue.rs.
 * =============================================================================
 */

//! Rescue toolkit types (errors, per-tool options, results).

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RescueError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Tool not available: {0}")]
    ToolNotAvailable(String),
    #[error("Job not found: {0}")]
    JobNotFound(String),
    #[error("Security error: {0}")]
    Security(String),
    /// A request field that is absent, malformed, or an option this
    /// integration cannot honour. Always a client error (400) naming the
    /// field, never a silent fallback.
    #[error("{0}")]
    InvalidInput(String),
    /// The admin cancelled the job; the tool was stopped. Distinguished from
    /// a failure so the UI doesn't show an error for a deliberate stop.
    #[error("Cancelled by admin")]
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDevice {
    pub device: String,
    pub name: String,
    pub size: u64,
    pub model: String,
    pub transport: String,
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DdrescueStatus {
    pub job_id: String,
    pub progress: f64,
    pub message: String,
    pub bytes_copied: Option<u64>,
    pub bytes_total: Option<u64>,
    pub speed: Option<String>,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDiskResult {
    pub device: String,
    pub partitions: Vec<PartitionEntry>,
    pub log: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionEntry {
    pub partition: String,
    pub size: String,
    pub filesystem: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoRecResult {
    pub device: String,
    pub output_dir: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdadmDiagnostic {
    pub array: String,
    pub level: String,
    pub state: String,
    pub devices: Vec<String>,
    pub size: String,
    pub health: String,
    pub rebuild_progress: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DdrescueOptions {
    pub logfile: Option<String>,
    pub block_size: Option<u32>,
    pub retry_passes: Option<u32>,
    pub direct_io: Option<bool>,
    pub reverse: Option<bool>,
    pub verbose: Option<bool>,
}

/// PhotoRec options. Deliberately only the two knobs that the packaged
/// PhotoRec's scripted (`/cmd`) mode can actually honour — see
/// `RescueService::photorec_recover` for why `keep_broken` and `max_size` are
/// no longer offered at all (they were accepted, stored, and ignored).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PhotoRecOptions {
    pub file_types: Option<Vec<String>>,
    pub partition_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MdadmOptions {
    pub action: Option<String>,
    pub force: Option<bool>,
    pub verbose: Option<bool>,
}

/// Default ddrescue block (sector) size in bytes.
pub const DEFAULT_BLOCK_SIZE: u32 = 512;
/// Hard bounds for `DdrescueOptions::block_size` (ddrescue requires a power of
/// two sector size; these are the meaningful ones for disks).
pub const MIN_BLOCK_SIZE: u32 = 512;
pub const MAX_BLOCK_SIZE: u32 = 1024 * 1024;
/// Upper bound for `retry_passes`.
pub const MAX_RETRY_PASSES: u32 = 100_000;

impl DdrescueOptions {
    /// The sector size to pass to `-b`, validated.
    pub fn validated_block_size(&self) -> Result<u32, RescueError> {
        let bs = self.block_size.unwrap_or(DEFAULT_BLOCK_SIZE);
        if !(MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&bs) {
            return Err(RescueError::InvalidInput(format!(
                "block_size must be between {MIN_BLOCK_SIZE} and {MAX_BLOCK_SIZE} bytes, got {bs}"
            )));
        }
        if !bs.is_power_of_two() {
            return Err(RescueError::InvalidInput(format!(
                "block_size must be a power of two, got {bs}"
            )));
        }
        Ok(bs)
    }

    /// Validate every option **before** the clone starts. These values are
    /// turned into argv, so a rejection is a 400 naming the field rather than
    /// a tool that fails (or behaves differently) halfway through.
    pub fn validate(&self) -> Result<(), RescueError> {
        self.validated_block_size()?;
        if let Some(retries) = self.retry_passes {
            if retries > MAX_RETRY_PASSES {
                return Err(RescueError::InvalidInput(format!(
                    "retry_passes must be at most {MAX_RETRY_PASSES}, got {retries}"
                )));
            }
        }
        Ok(())
    }
}

/// PhotoRec partition-table keyword accepted by the scripted `/cmd` mode.
pub fn partition_type_keyword(value: &str) -> Option<&'static str> {
    match value {
        "Intel" => Some("partition_i386"),
        "EFI" => Some("partition_gpt"),
        "Mac" => Some("partition_mac"),
        "Sun" => Some("partition_sun"),
        "None" => Some("partition_none"),
        _ => None,
    }
}

impl PhotoRecOptions {
    /// Validate the `/cmd` script inputs.
    ///
    /// `file_types` are spliced **into PhotoRec's comma-separated command
    /// grammar** (`fileopt,everything,disable,jpg,enable,…`), so a value
    /// containing a comma, whitespace or a keyword would inject extra
    /// commands into the scripted run. Only bare lower-case extension
    /// identifiers are accepted.
    pub fn validate(&self) -> Result<(), RescueError> {
        if let Some(types) = &self.file_types {
            if types.len() > 64 {
                return Err(RescueError::InvalidInput(format!(
                    "at most 64 file types may be requested, got {}",
                    types.len()
                )));
            }
            for ft in types {
                let ok = !ft.is_empty()
                    && ft.len() <= 32
                    && ft
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                if !ok {
                    return Err(RescueError::InvalidInput(format!(
                        "invalid file type '{ft}': expected a bare lower-case extension such as 'jpg'"
                    )));
                }
            }
        }

        if let Some(pt) = &self.partition_type {
            if partition_type_keyword(pt).is_none() {
                return Err(RescueError::InvalidInput(format!(
                    "unknown partition_type '{pt}': expected one of Intel, EFI, Mac, Sun, None"
                )));
            }
        }

        Ok(())
    }
}

/// mdadm `--action` values this integration accepts.
///
/// `rewrite` is the frontend's label for a write-correcting sync action and
/// maps to `repair`; anything else is refused instead of being forwarded to
/// mdadm as `--action=<garbage>` (which is what the previous
/// `Some(other) => other` arm did).
pub const MDADM_ACTIONS: &[&str] = &[
    "repair", "check", "idle", "frozen", "resync", "recover", "reshape",
];

impl MdadmOptions {
    /// The `--action=` value to use, validated.
    pub fn validated_action(&self) -> Result<&str, RescueError> {
        match self.action.as_deref() {
            None | Some("") => Ok("repair"),
            Some("rewrite") => Ok("repair"),
            Some(other) if MDADM_ACTIONS.contains(&other) => Ok(other),
            Some(other) => Err(RescueError::InvalidInput(format!(
                "unknown mdadm action '{other}': expected one of repair, check, rewrite, idle, frozen, resync, recover, reshape"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddrescue_block_size_defaults_to_512() {
        let opts = DdrescueOptions::default();
        assert_eq!(opts.validated_block_size().unwrap(), 512);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn ddrescue_block_size_must_be_a_power_of_two_in_range() {
        for good in [512u32, 1024, 4096, 65536, 1_048_576] {
            let opts = DdrescueOptions {
                block_size: Some(good),
                ..Default::default()
            };
            assert_eq!(opts.validated_block_size().unwrap(), good);
        }
        for bad in [0u32, 1, 1000, 2048 * 3, 2_097_152] {
            let opts = DdrescueOptions {
                block_size: Some(bad),
                ..Default::default()
            };
            assert!(
                opts.validated_block_size().is_err(),
                "block size {bad} must be refused"
            );
        }
    }

    #[test]
    fn ddrescue_retry_passes_is_bounded() {
        let opts = DdrescueOptions {
            retry_passes: Some(MAX_RETRY_PASSES + 1),
            ..Default::default()
        };
        assert!(opts.validate().is_err());

        let opts = DdrescueOptions {
            retry_passes: Some(3),
            ..Default::default()
        };
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn photorec_accepts_plain_extensions_and_known_partition_types() {
        let opts = PhotoRecOptions {
            file_types: Some(vec![
                "jpg".into(),
                "pdf".into(),
                "mp4".into(),
                "sqlite".into(),
            ]),
            partition_type: Some("EFI".into()),
        };
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn photorec_rejects_script_injection_in_file_types() {
        // These land in PhotoRec's comma-separated /cmd grammar, so anything
        // with a comma/space/upper case/keyword could add commands.
        for bad in [
            "jpg,fileopt,everything,enable",
            "jpg,search",
            "JPG",
            "jp g",
            "",
            "../etc",
            "jpg;rm",
        ] {
            let opts = PhotoRecOptions {
                file_types: Some(vec![bad.to_string()]),
                partition_type: None,
            };
            assert!(
                opts.validate().is_err(),
                "file type {bad:?} must be refused"
            );
        }
    }

    #[test]
    fn photorec_rejects_unknown_partition_type() {
        let opts = PhotoRecOptions {
            file_types: None,
            partition_type: Some("Amiga".into()),
        };
        assert!(opts.validate().is_err());
    }

    #[test]
    fn mdadm_action_is_whitelisted_and_rewrite_maps_to_repair() {
        let default = MdadmOptions::default();
        assert_eq!(default.validated_action().unwrap(), "repair");

        let rewrite = MdadmOptions {
            action: Some("rewrite".into()),
            ..Default::default()
        };
        assert_eq!(rewrite.validated_action().unwrap(), "repair");

        for good in MDADM_ACTIONS {
            let opts = MdadmOptions {
                action: Some((*good).into()),
                ..Default::default()
            };
            assert_eq!(opts.validated_action().unwrap(), *good);
        }

        for bad in ["delete", "repair --force", "", "IDLE"] {
            let opts = MdadmOptions {
                action: Some(bad.into()),
                ..Default::default()
            };
            let result = opts.validated_action();
            if bad.is_empty() {
                assert_eq!(result.unwrap(), "repair", "an empty action is the default");
            } else {
                assert!(result.is_err(), "action {bad:?} must be refused");
            }
        }
    }
}
