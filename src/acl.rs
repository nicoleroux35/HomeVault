/* =============================================================================
 * File          : acl.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   POSIX filesystem ACL management via the setfacl/getfacl tools: grant
 *   or revoke per-user/per-group access on share trees (optionally
 *   recursive), and read the effective entries back.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2)
 *
 * Usage:
 *   permissions.rs (the higher-level grant/revoke logic) and the share
 *   permission endpoints.
 * =============================================================================
 */

//! POSIX ACL management via the setfacl/getfacl tools.

use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

/// Errors that can occur during ACL operations.
#[derive(Error, Debug)]
pub enum AclError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Path not found: {0}")]
    NotFound(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Invalid permission: {0}")]
    InvalidPermission(String),
}

/// A single ACL entry consisting of a tag, optional qualifier (user/group name), and permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclEntry {
    pub tag: AclTag,
    pub qualifier: Option<String>,
    pub permissions: AclPermissions,
}

/// ACL tag type: user, group, mask, or other.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AclTag {
    User,
    Group,
    Mask,
    Other,
}

impl std::fmt::Display for AclTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AclTag::User => write!(f, "user"),
            AclTag::Group => write!(f, "group"),
            AclTag::Mask => write!(f, "mask"),
            AclTag::Other => write!(f, "other"),
        }
    }
}

impl From<&str> for AclTag {
    fn from(s: &str) -> Self {
        match s {
            "user" => AclTag::User,
            "group" => AclTag::Group,
            "mask" => AclTag::Mask,
            "other" => AclTag::Other,
            _ => AclTag::Other,
        }
    }
}

/// Read, write, and execute permission flags.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AclPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl AclPermissions {
    /// Parse a 3-character permission string (e.g. `"rwx"`) into flags.
    pub fn from_string(s: &str) -> Result<Self, AclError> {
        if s.len() != 3 {
            return Err(AclError::InvalidPermission(s.to_string()));
        }

        let chars: Vec<char> = s.chars().collect();
        Ok(Self {
            read: chars[0] == 'r' || chars[0] == 'R',
            write: chars[1] == 'w' || chars[1] == 'W',
            execute: chars[2] == 'x' || chars[2] == 'X',
        })
    }

    /// Convert permission flags to an `rwx` string.
    pub fn to_rwx_string(&self) -> String {
        format!(
            "{}{}{}",
            if self.read { "r" } else { "-" },
            if self.write { "w" } else { "-" },
            if self.execute { "x" } else { "-" }
        )
    }

    /// Parse permission flags from a Unix mode bitset (owner section).
    pub fn from_mode(mode: u32) -> Self {
        Self {
            read: (mode & 0o400) != 0,
            write: (mode & 0o200) != 0,
            execute: (mode & 0o100) != 0,
        }
    }
}

impl std::fmt::Display for AclPermissions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_rwx_string())
    }
}

/// Full ACL information for a filesystem path, including default entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAcl {
    pub path: String,
    pub entries: Vec<AclEntry>,
    pub default_entries: Vec<AclEntry>,
}

/// Service for managing POSIX ACLs on files and directories via `getfacl`/`setfacl`.
pub struct AclService;

impl AclService {
    // Directories where ACL operations are permitted
    fn allowed_acl_prefixes() -> Vec<String> {
        vec![
            crate::config::paths::HOMES_BASE_DIR.to_string(),
            "/mnt/".to_string(),
            "/srv/".to_string(),
        ]
    }

    /// Validate a path is safe for ACL operations: exists, resolves to a canonical path,
    /// is not a system path, and is within the allowed directory whitelist.
    fn validate_acl_path(path: &str) -> Result<(), AclError> {
        if !std::path::Path::new(path).exists() {
            return Err(AclError::NotFound(path.to_string()));
        }

        // Resolve canonical path to prevent path traversal (.., symlinks)
        let canonical = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => return Err(AclError::NotFound(path.to_string())),
        };

        let canonical_str = canonical.to_string_lossy();

        // Block sensitive system paths
        const FORBIDDEN_PATHS: &[&str] = &[
            "/etc/",
            "/root/",
            "/boot/",
            "/sys/",
            "/proc/",
            "/dev/",
            "/bin/",
            "/sbin/",
            "/usr/bin/",
            "/usr/sbin/",
            "/lib/",
            "/lib64/",
        ];

        for forbidden in FORBIDDEN_PATHS {
            if canonical_str.starts_with(forbidden) {
                return Err(AclError::CommandFailed(format!(
                    "Access denied: '{}' is a protected system path",
                    path
                )));
            }
        }

        // Enforce whitelist of allowed parent directories
        let allowed = Self::allowed_acl_prefixes();
        let is_allowed = allowed
            .iter()
            .any(|prefix| canonical_str.starts_with(prefix));

        if !is_allowed {
            return Err(AclError::CommandFailed(format!(
                "Path '{}' is outside allowed directories. Allowed: {:?}",
                path, allowed
            )));
        }

        Ok(())
    }

    /// Return the full ACL (access + default entries) for a path.
    pub fn get_acl(path: &str) -> Result<FileAcl, AclError> {
        Self::validate_acl_path(path)?;

        let output = Command::new("getfacl").arg(path).output()?;

        if !output.status.success() {
            return Err(AclError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();
        let mut default_entries = Vec::new();
        let mut in_default = false;

        for line in stdout.lines() {
            let line = line.trim();

            if line.starts_with("# file:") {
                continue;
            }

            if line == "# default:" {
                in_default = true;
                continue;
            }

            if line.starts_with('#') || line.is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 3 {
                continue;
            }

            let tag = AclTag::from(parts[0]);
            let qualifier = if parts[0] == "user" || parts[0] == "group" {
                Some(parts[1].to_string())
            } else {
                None
            };

            let perm_str = if qualifier.is_some() {
                parts[2]
            } else {
                parts[1]
            };
            let permissions = AclPermissions::from_string(perm_str)?;

            let entry = AclEntry {
                tag,
                qualifier,
                permissions,
            };

            if in_default {
                default_entries.push(entry);
            } else {
                entries.push(entry);
            }
        }

        Ok(FileAcl {
            path: path.to_string(),
            entries,
            default_entries,
        })
    }

    /// Set ACL entries on a path via `setfacl -m`.
    pub fn set_acl(path: &str, entries: &[AclEntry]) -> Result<(), AclError> {
        Self::validate_acl_path(path)?;

        for entry in entries {
            let tag_str = entry.tag.to_string();
            let perm_str = entry.permissions.to_string();

            let (user_or_group, qualifier) = if let Some(ref q) = entry.qualifier {
                (tag_str.as_str(), q.as_str())
            } else {
                (tag_str.as_str(), "")
            };

            let mut args = vec!["-m".to_string()];

            if entry.tag == AclTag::User || entry.tag == AclTag::Group {
                args.push(format!("{}:{}:{}", user_or_group, qualifier, perm_str));
            } else {
                args.push(format!("{}:{}", user_or_group, perm_str));
            }

            args.push(path.to_string());

            let output = Command::new("sudo").arg("setfacl").args(&args).output()?;

            if !output.status.success() {
                return Err(AclError::CommandFailed(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Set default ACL entries on a path (inherited by new children).
    pub fn set_default_acl(path: &str, entries: &[AclEntry]) -> Result<(), AclError> {
        Self::validate_acl_path(path)?;

        for entry in entries {
            let tag_str = entry.tag.to_string();
            let perm_str = entry.permissions.to_string();

            let (user_or_group, qualifier) = if let Some(ref q) = entry.qualifier {
                (tag_str.as_str(), q.as_str())
            } else {
                (tag_str.as_str(), "")
            };

            // `-d` must come before `-m`: setfacl's arg parser takes the very
            // next argv element as `-m`'s value regardless of a leading dash,
            // so `-m -d spec` feeds it the literal string "-d" as the ACL
            // spec ("Option -m: Invalid argument") instead of enabling
            // default-ACL mode. Confirmed against a real `setfacl` — `-m -d`
            // fails, `-d -m` succeeds.
            let mut args = vec!["-d".to_string(), "-m".to_string()];

            if entry.tag == AclTag::User || entry.tag == AclTag::Group {
                args.push(format!("{}:{}:{}", user_or_group, qualifier, perm_str));
            } else {
                args.push(format!("{}:{}", user_or_group, perm_str));
            }

            args.push(path.to_string());

            let output = Command::new("sudo").arg("setfacl").args(&args).output()?;

            if !output.status.success() {
                return Err(AclError::CommandFailed(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Remove a specific ACL entry (by tag + qualifier) from a path.
    pub fn remove_acl(path: &str, tag: &AclTag, qualifier: Option<&str>) -> Result<(), AclError> {
        Self::validate_acl_path(path)?;

        let tag_str = tag.to_string();
        let spec = if tag == &AclTag::User || tag == &AclTag::Group {
            match qualifier {
                Some(q) => format!("{}:{}:", tag_str, q),
                None => {
                    return Err(AclError::InvalidPermission(
                        "Qualifier required for user/group".to_string(),
                    ))
                }
            }
        } else {
            format!("{}:", tag_str)
        };

        // Remove the access entry. A grant that was `recursive` also set a
        // matching *default* ACL entry (`default:user:X:…`), so remove that
        // too — otherwise a revoked user keeps being granted on every file
        // created in the share afterwards. The default removal is
        // best-effort: `setfacl -x` errors if the entry isn't there (a
        // non-recursive grant), which must not fail the revoke.
        let output = Command::new("sudo")
            .arg("setfacl")
            .args(["-x", spec.as_str(), path])
            .output()?;
        if !output.status.success() {
            return Err(AclError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let _ = Command::new("sudo")
            .arg("setfacl")
            .args(["-d", "-x", spec.as_str(), path])
            .output();

        Ok(())
    }

    /// Remove all ACL entries from a path via `setfacl -b`.
    pub fn remove_all_acl(path: &str) -> Result<(), AclError> {
        Self::validate_acl_path(path)?;

        let output = Command::new("sudo")
            .arg("setfacl")
            .args(["-b", path])
            .output()?;

        if !output.status.success() {
            return Err(AclError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Copy ACL from one path to another (clears destination first).
    pub fn copy_acl(source: &str, dest: &str) -> Result<(), AclError> {
        let acl = Self::get_acl(source)?;

        Self::remove_all_acl(dest)?;

        Self::set_acl(dest, &acl.entries)?;

        if !acl.default_entries.is_empty() {
            Self::set_default_acl(dest, &acl.default_entries)?;
        }

        Ok(())
    }

    /// Return `true` if the system has `getfacl` and `setfacl` available.
    pub fn check_acl_support() -> bool {
        Command::new("which")
            .arg("getfacl")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            && Command::new("which")
                .arg("setfacl")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }

    pub fn get_posix_permissions(path: &str) -> Result<(u32, String), AclError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(path)?;
            let mode = metadata.permissions().mode() & 0o7777;
            let perms_str = Self::format_mode(mode);
            Ok((mode, perms_str))
        }

        #[cfg(not(unix))]
        {
            let _ = path;
            Err(AclError::CommandFailed(
                "POSIX permissions not supported on this platform".to_string(),
            ))
        }
    }

    pub fn set_posix_permissions(path: &str, mode: u32) -> Result<(), AclError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(path)?;
            let mut perms = metadata.permissions();
            perms.set_mode(mode);
            std::fs::set_permissions(path, perms)?;
            Ok(())
        }

        #[cfg(not(unix))]
        {
            let _ = path;
            let _ = mode;
            Err(AclError::CommandFailed(
                "POSIX permissions not supported on this platform".to_string(),
            ))
        }
    }

    #[allow(dead_code)]
    fn format_mode(mode: u32) -> String {
        let chars = [
            (mode & 0o400 != 0, mode & 0o200 != 0, mode & 0o100 != 0),
            (mode & 0o040 != 0, mode & 0o020 != 0, mode & 0o010 != 0),
            (mode & 0o004 != 0, mode & 0o002 != 0, mode & 0o001 != 0),
        ];

        let owner = format!(
            "{}{}{}",
            if chars[0].0 { "r" } else { "-" },
            if chars[0].1 { "w" } else { "-" },
            if chars[0].2 { "x" } else { "-" }
        );

        let group = format!(
            "{}{}{}",
            if chars[1].0 { "r" } else { "-" },
            if chars[1].1 { "w" } else { "-" },
            if chars[1].2 { "x" } else { "-" }
        );

        let other = format!(
            "{}{}{}",
            if chars[2].0 { "r" } else { "-" },
            if chars[2].1 { "w" } else { "-" },
            if chars[2].2 { "x" } else { "-" }
        );

        format!("{}{}{}", owner, group, other)
    }
}
