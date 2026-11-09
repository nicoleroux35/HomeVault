/* =============================================================================
 * File          : syncthing_versions.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   File-version browsing for Syncthing folders: the versioning policy a
 *   folder keeps (trash can / simple / staggered), the archived versions the
 *   daemon reports, and the validation of a restore request.
 *
 *   Syncthing already owns the mechanism — it moves the replaced file into
 *   `.stversions` and keeps N of them. HomeVault does not implement a second
 *   version store; it reads the daemon's own policy, lets the admin change it,
 *   lists what is archived, and asks the daemon to restore one version. Every
 *   shape below is pure and unit-tested.
 *
 * Dependencies:
 *   - serde (>= 1.0), serde_json (>= 1.0), chrono (>= 0.4)
 *
 * Usage:
 *   handlers::syncthing exposes /api/syncthing/folders/{id}/versions; see
 *   docs/guides/syncthing.md.
 * =============================================================================
 */

//! File versions for Syncthing folders (see docs/guides/syncthing.md).

use serde::{Deserialize, Serialize};

/// Largest accepted `keep` value. Syncthing accepts more; above this the
/// retention is a disk-space decision the admin should make deliberately, and
/// a UI typo (`5000`) would silently eat the pool.
pub const MAX_KEEP: u32 = 1000;

/// Default number of versions kept when the admin turns versioning on.
pub const DEFAULT_KEEP: u32 = 5;

/// Largest accepted "delete versions older than N days" value.
pub const MAX_CLEANOUT_DAYS: u32 = 3650;

/// Largest accepted staggered `maxAge` (in days).
pub const MAX_AGE_DAYS: u32 = 3650;

/// Versions returned by one listing.
pub const MAX_LISTED: usize = 500;

/// How a folder keeps old copies of a replaced file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersioningMode {
    /// No versioning: a replaced file is gone.
    Off,
    /// Replaced files sit in `.stversions` until the cleanout period ends.
    Trashcan,
    /// Keep the last N versions of each file.
    Simple,
    /// Keep versions until a per-file age limit, thinned out over time.
    Staggered,
}

impl Default for VersioningMode {
    fn default() -> Self {
        Self::Off
    }
}

impl VersioningMode {
    /// The value Syncthing stores in `versioning.type` (`""` = off).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "",
            Self::Trashcan => "trashcan",
            Self::Simple => "simple",
            Self::Staggered => "staggered",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "none" => Some(Self::Off),
            "trashcan" => Some(Self::Trashcan),
            "simple" => Some(Self::Simple),
            "staggered" => Some(Self::Staggered),
            "external" => None,
            _ => None,
        }
    }

    /// Whether this mode keeps a count (`keep`) rather than an age.
    pub fn keeps_count(self) -> bool {
        matches!(self, Self::Simple)
    }
}

/// One folder's versioning policy, as the UI shows and edits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersioningSettings {
    pub mode: VersioningMode,
    /// `simple`: how many versions of each file to keep.
    #[serde(default = "default_keep")]
    pub keep: u32,
    /// Every mode except off: delete versions older than this (0 = never).
    #[serde(default)]
    pub cleanout_days: u32,
    /// `staggered`: versions older than this are dropped (0 = never).
    #[serde(default)]
    pub max_age_days: u32,
}

fn default_keep() -> u32 {
    DEFAULT_KEEP
}

impl Default for VersioningSettings {
    fn default() -> Self {
        Self {
            mode: VersioningMode::Off,
            keep: DEFAULT_KEEP,
            cleanout_days: 0,
            max_age_days: 0,
        }
    }
}

impl VersioningSettings {
    /// Refuse a policy Syncthing would accept but that cannot be what the
    /// admin meant (0 versions, a 5000-version retention, a negative age).
    pub fn validate(&self) -> Result<(), String> {
        if self.keep == 0 || self.keep > MAX_KEEP {
            return Err(format!(
                "Keep between 1 and {MAX_KEEP} versions of each file"
            ));
        }
        if self.cleanout_days > MAX_CLEANOUT_DAYS {
            return Err(format!(
                "Delete versions older than at most {MAX_CLEANOUT_DAYS} days"
            ));
        }
        if self.max_age_days > MAX_AGE_DAYS {
            return Err(format!("Keep versions for at most {MAX_AGE_DAYS} days"));
        }
        Ok(())
    }

    /// The `versioning` object Syncthing stores in a folder's config.
    ///
    /// `params` is a `map[string]string` in Syncthing's config, so every value
    /// is a decimal string — never a number.
    pub fn to_config(self) -> serde_json::Value {
        let mut params = serde_json::Map::new();
        match self.mode {
            VersioningMode::Off => {
                return serde_json::json!({ "type": "" });
            }
            VersioningMode::Trashcan => {
                params.insert(
                    "cleanoutDays".to_string(),
                    serde_json::json!(self.cleanout_days.to_string()),
                );
            }
            VersioningMode::Simple => {
                params.insert("keep".to_string(), serde_json::json!(self.keep.to_string()));
                params.insert(
                    "cleanoutDays".to_string(),
                    serde_json::json!(self.cleanout_days.to_string()),
                );
            }
            VersioningMode::Staggered => {
                params.insert(
                    "maxAge".to_string(),
                    serde_json::json!((self.max_age_days as u64 * 86_400).to_string()),
                );
                params.insert(
                    "cleanoutDays".to_string(),
                    serde_json::json!(self.cleanout_days.to_string()),
                );
            }
        }
        serde_json::json!({
            "type": self.mode.as_str(),
            "params": serde_json::Value::Object(params),
        })
    }

    /// Read a folder's `versioning` object back into the UI shape.
    ///
    /// Tolerant on purpose: an `external` versioner (or one this build does not
    /// know) reads as `Off` **with its raw type preserved in the log**, rather
    /// than pretending to edit a policy HomeVault does not understand.
    pub fn from_config(value: &serde_json::Value) -> Self {
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let Some(mode) = VersioningMode::parse(kind) else {
            tracing::debug!("syncthing: folder uses an unknown versioner '{kind}'");
            return Self::default();
        };
        if mode == VersioningMode::Off {
            return Self::default();
        }
        let param = |name: &str| -> Option<u64> {
            value
                .get("params")
                .and_then(|p| p.get(name))
                .and_then(|v| match v {
                    serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
                    serde_json::Value::Number(n) => n.as_u64(),
                    _ => None,
                })
        };
        let keep = param("keep")
            .unwrap_or(DEFAULT_KEEP as u64)
            .clamp(1, MAX_KEEP as u64) as u32;
        let cleanout_days = param("cleanoutDays")
            .unwrap_or(0)
            .min(MAX_CLEANOUT_DAYS as u64) as u32;
        let max_age_days = param("maxAge")
            .map(|seconds| (seconds / 86_400).min(MAX_AGE_DAYS as u64) as u32)
            .unwrap_or(0);
        Self {
            mode,
            keep,
            cleanout_days,
            max_age_days,
        }
    }
}

/// One archived version of one file, as the daemon reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedVersion {
    /// Path relative to the folder (`docs/notes.txt`).
    pub file: String,
    /// RFC 3339 stamp Syncthing assigns (`versionTime`).
    pub version_time: String,
    /// The file's own modification time when it was archived, when reported.
    #[serde(default)]
    pub mod_time: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Parse `GET /rest/folder/versions` output.
///
/// The endpoint returns one object per archived file. Only `name` and
/// `versionTime` are load-bearing for a restore; a row missing either is
/// skipped rather than shown as un-restorable.
pub fn parse_versions(raw: &str) -> Result<Vec<ArchivedVersion>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("Could not read the versions: {e}"))?;
    let Some(rows) = value.as_array() else {
        return Err("The versions response is not a list".to_string());
    };
    let mut out = Vec::new();
    for row in rows {
        let Some(file) = row.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(version_time) = row.get("versionTime").and_then(|v| v.as_str()) else {
            continue;
        };
        if file.is_empty() || version_time.is_empty() {
            continue;
        }
        out.push(ArchivedVersion {
            file: file.to_string(),
            version_time: version_time.to_string(),
            mod_time: row
                .get("modTime")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            size: row.get("size").and_then(|v| v.as_u64()),
        });
        if out.len() >= MAX_LISTED {
            break;
        }
    }
    // Newest first: the version a user is most likely to want is on top.
    out.sort_by(|a, b| b.version_time.cmp(&a.version_time));
    Ok(out)
}

/// Validate one restore request. The file is a path **inside** the folder, and
/// Syncthing resolves it there itself — a traversal is refused here so the
/// daemon is never asked to restore `../etc/shadow`.
pub fn validate_restore(file: &str, version_time: &str) -> Result<(), String> {
    let file = file.trim();
    if file.is_empty() || file.len() > 4096 {
        return Err("Choose a file version to restore".to_string());
    }
    if file.chars().any(|c| c.is_control() || c == '\0') {
        return Err("Invalid file name".to_string());
    }
    if file.starts_with('/') || file.contains("..") || file.contains('\\') {
        return Err("Invalid file name".to_string());
    }
    if version_time.trim().is_empty() {
        return Err("Choose a version to restore".to_string());
    }
    chrono::DateTime::parse_from_rfc3339(version_time.trim())
        .map_err(|_| "That version stamp is not a valid date".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_shape_matches_syncthing() {
        // Off: an empty type, no params.
        assert_eq!(
            VersioningSettings::default().to_config(),
            serde_json::json!({ "type": "" })
        );

        // Simple: keep + cleanout, both decimal strings.
        let simple = VersioningSettings {
            mode: VersioningMode::Simple,
            keep: 10,
            cleanout_days: 30,
            max_age_days: 0,
        };
        let value = simple.to_config();
        assert_eq!(value["type"], "simple");
        assert_eq!(value["params"]["keep"], "10");
        assert_eq!(value["params"]["cleanoutDays"], "30");

        // Staggered: maxAge is in seconds, not days.
        let staggered = VersioningSettings {
            mode: VersioningMode::Staggered,
            keep: 5,
            cleanout_days: 0,
            max_age_days: 7,
        };
        assert_eq!(staggered.to_config()["params"]["maxAge"], "604800");

        // Trash can: only the cleanout period.
        let trash = VersioningSettings {
            mode: VersioningMode::Trashcan,
            keep: 5,
            cleanout_days: 15,
            max_age_days: 0,
        };
        let value = trash.to_config();
        assert_eq!(value["type"], "trashcan");
        assert_eq!(value["params"]["cleanoutDays"], "15");
        assert!(value["params"].get("keep").is_none());
    }

    #[test]
    fn test_config_round_trip_and_unknown_versioner() {
        for settings in [
            VersioningSettings::default(),
            VersioningSettings {
                mode: VersioningMode::Simple,
                keep: 7,
                cleanout_days: 90,
                max_age_days: 0,
            },
            VersioningSettings {
                mode: VersioningMode::Staggered,
                keep: 5,
                cleanout_days: 0,
                max_age_days: 14,
            },
        ] {
            let back = VersioningSettings::from_config(&settings.to_config());
            assert_eq!(back, settings);
        }

        // An `external` versioner (a script we do not manage) reads as off
        // instead of being half-edited.
        let external = serde_json::json!({ "type": "external", "params": { "command": "x" } });
        assert_eq!(
            VersioningSettings::from_config(&external).mode,
            VersioningMode::Off
        );
        // A missing object is off too.
        assert_eq!(
            VersioningSettings::from_config(&serde_json::json!({})).mode,
            VersioningMode::Off
        );
    }

    #[test]
    fn test_policy_bounds() {
        let mut settings = VersioningSettings {
            mode: VersioningMode::Simple,
            keep: 0,
            cleanout_days: 0,
            max_age_days: 0,
        };
        assert!(settings.validate().is_err(), "0 versions is a mistake");
        settings.keep = MAX_KEEP + 1;
        assert!(settings.validate().is_err());
        settings.keep = 5;
        settings.cleanout_days = MAX_CLEANOUT_DAYS + 1;
        assert!(settings.validate().is_err());
        settings.cleanout_days = 0;
        settings.max_age_days = MAX_AGE_DAYS + 1;
        assert!(settings.validate().is_err());
        settings.max_age_days = 0;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn test_parse_versions_sorts_newest_first_and_skips_broken_rows() {
        let raw = r#"[
            {"name":"docs/a.txt","versionTime":"2026-11-01T10:00:00Z","size":10},
            {"name":"docs/a.txt","versionTime":"2026-11-09T10:00:00Z","modTime":"2026-11-08T09:00:00Z"},
            {"name":"no-time"},
            {"versionTime":"2026-11-01T10:00:00Z"},
            {"name":"","versionTime":"2026-11-01T10:00:00Z"}
        ]"#;
        let versions = parse_versions(raw).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version_time, "2026-11-09T10:00:00Z");
        assert_eq!(versions[0].file, "docs/a.txt");
        assert_eq!(versions[1].size, Some(10));
        assert!(versions[0].size.is_none());
        assert_eq!(
            versions[0].mod_time.as_deref(),
            Some("2026-11-08T09:00:00Z")
        );

        assert!(parse_versions("not json").is_err());
        assert!(parse_versions(r#"{"a":1}"#).is_err());
        assert!(parse_versions("[]").unwrap().is_empty());
    }

    #[test]
    fn test_restore_validation() {
        assert!(validate_restore("docs/a.txt", "2026-11-09T10:00:00Z").is_ok());
        for (file, stamp) in [
            ("", "2026-11-09T10:00:00Z"),
            ("../etc/shadow", "2026-11-09T10:00:00Z"),
            ("/etc/shadow", "2026-11-09T10:00:00Z"),
            ("a\\b", "2026-11-09T10:00:00Z"),
            ("a.txt", ""),
            ("a.txt", "yesterday"),
        ] {
            assert!(
                validate_restore(file, stamp).is_err(),
                "{file:?} / {stamp:?} must be refused"
            );
        }
    }

    #[test]
    fn test_mode_parsing() {
        assert_eq!(VersioningMode::parse(""), Some(VersioningMode::Off));
        assert_eq!(VersioningMode::parse("OFF"), Some(VersioningMode::Off));
        assert_eq!(
            VersioningMode::parse("simple"),
            Some(VersioningMode::Simple)
        );
        assert_eq!(
            VersioningMode::parse("staggered"),
            Some(VersioningMode::Staggered)
        );
        assert_eq!(
            VersioningMode::parse("trashcan"),
            Some(VersioningMode::Trashcan)
        );
        assert_eq!(VersioningMode::parse("external"), None);
        assert!(VersioningMode::Simple.keeps_count());
        assert!(!VersioningMode::Staggered.keeps_count());
    }
}
