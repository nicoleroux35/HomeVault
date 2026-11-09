/* =============================================================================
 * File          : update_engine.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   "Apply update" from the web UI (P2-8, UI half): the running server queues
 *   a signed release manifest, a fixed systemd unit runs the installer engine
 *   as root, and the UI follows the outcome.
 *
 *   The server itself never performs the swap. It writes a small request
 *   (`data/updates/request.json`, mode 0600) and asks systemd to start
 *   `homevault-update.service`, whose ExecStart is the *fixed* argv
 *   `<install>/homevault --update-engine`. A compromised HomeVault process can
 *   therefore only ask for an update — and the installer still refuses any
 *   manifest the release key did not sign, before it touches a single file.
 *
 *   The engine runs detached from the service it replaces: the installer
 *   restarts `homevault.service` in the middle of the swap, and the request
 *   that started it would otherwise be killed by its own success.
 *
 * Dependencies:
 *   - serde (>= 1.0), serde_json (>= 1.0), crate::utils::process,
 *     crate::config::paths
 *
 * Usage:
 *   handlers::update_apply exposes the /api/settings/update routes; the engine
 *   is the `--update-engine` mode in src/bin/server.rs.
 * =============================================================================
 */

//! Signed-update application from the web UI (see docs/guides/settings.md).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Where the installer keeps its engine on an installed appliance. The
/// installer copies itself there (and the unit's `ConditionPathExists` uses the
/// same path), so a `curl | bash` install still offers the button.
pub const INSTALLER_PATH: &str = "/opt/homevault/installer/install.sh";

/// The root-owned systemd unit that runs the engine. `repr` is deliberately
/// fixed: the sudoers grant names this one unit, never a parameterised
/// command.
pub const UPDATE_UNIT: &str = "homevault-update";

/// Deadline for `systemctl start`. Starting a oneshot unit returns at once;
/// a `systemctl` that has not answered in twenty seconds is wedged.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);

/// How many bytes of the engine log the status endpoint returns.
pub const LOG_TAIL_BYTES: usize = 16 * 1024;

/// One queued update request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRequest {
    /// The signed release manifest (https URL, or a path the operator placed
    /// on the appliance).
    pub manifest: String,
    /// Detached signature URL/path; the installer defaults to `<manifest>.sig`.
    #[serde(default)]
    pub signature: Option<String>,
    /// Release trust anchor (PEM path). Absent = the installer's default
    /// (`/usr/local/lib/homevault/release-key.pem`).
    #[serde(default)]
    pub pubkey: Option<String>,
    /// Who asked, for the engine log and the event trail.
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub requested_at: Option<String>,
}

/// What the last engine run exited with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineExit {
    /// `ok`, `failed` or `unknown`.
    pub status: String,
    pub exit_code: Option<i32>,
    pub finished_at: String,
}

/// `data/updates` (created on demand).
pub fn updates_dir() -> PathBuf {
    crate::config::paths::base_dir().join("updates")
}

/// `data/updates/request.json`.
pub fn request_path() -> PathBuf {
    updates_dir().join("request.json")
}

/// `data/updates/apply.log`.
pub fn log_path() -> PathBuf {
    updates_dir().join("apply.log")
}

/// `data/updates/engine-exit.json`.
pub fn exit_path() -> PathBuf {
    updates_dir().join("engine-exit.json")
}

/// The installer engine on this host, when it is there.
pub fn installer_path() -> Option<PathBuf> {
    let path = PathBuf::from(INSTALLER_PATH);
    path.is_file().then_some(path)
}

/// Whether the UI can offer the button at all: the engine script and the unit
/// must both be installed.
pub fn available() -> bool {
    installer_path().is_some() && unit_installed()
}

/// The unit file exists (`/etc/systemd/system/homevault-update.service`).
pub fn unit_installed() -> bool {
    Path::new(&format!("/etc/systemd/system/{UPDATE_UNIT}.service")).is_file()
}

/// A release manifest is something the installer can verify: an https URL, or
/// an absolute path. Everything else (http, `file://`, a shell-looking string)
/// is refused — the value is handed to the installer as an environment
/// variable, and the signature check is the only thing standing between a
/// wrong value and a replaced binary.
pub fn validate_manifest(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("A signed release manifest is required".to_string());
    }
    if value.len() > 2048 {
        return Err("The release manifest address is too long".to_string());
    }
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("The release manifest address must not contain spaces".to_string());
    }
    if value.starts_with("https://") {
        return Ok(value.to_string());
    }
    if value.starts_with('/') && !value.contains("..") {
        return Ok(value.to_string());
    }
    Err("Use an https:// address (or an absolute path) for the signed release manifest".to_string())
}

/// The optional detached signature follows the same rule as the manifest.
pub fn validate_signature(raw: &str) -> Result<Option<String>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    validate_manifest(value).map(Some)
}

/// The optional trust anchor must be an absolute path (a PEM file on disk;
/// there is no such thing as a remote key here).
pub fn validate_pubkey(raw: &str) -> Result<Option<String>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if !value.starts_with('/') || value.contains("..") || value.chars().any(char::is_whitespace) {
        return Err("The release public key must be an absolute path".to_string());
    }
    Ok(Some(value.to_string()))
}

/// Write the request atomically with mode 0600 (it names the release channel
/// and, optionally, a key path).
pub fn write_request(request: &UpdateRequest) -> Result<(), String> {
    let path = request_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_vec_pretty(request).map_err(|e| e.to_string())?;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.write_all(&body)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.sync_all().ok();
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read the queued request, if any.
pub fn read_request() -> Option<UpdateRequest> {
    let raw = std::fs::read_to_string(request_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Drop the request once the engine consumed it (or the caller withdrew it).
pub fn clear_request() {
    let _ = std::fs::remove_file(request_path());
}

/// The last engine exit, when it ran.
pub fn read_exit() -> Option<EngineExit> {
    let raw = std::fs::read_to_string(exit_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_exit(exit: &EngineExit) {
    let path = exit_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_vec_pretty(exit) {
        let _ = std::fs::write(&path, body);
    }
}

/// The tail of the engine log (last [`LOG_TAIL_BYTES`]), best effort.
pub fn log_tail() -> String {
    let raw = match std::fs::read(log_path()) {
        Ok(raw) => raw,
        Err(_) => return String::new(),
    };
    let start = raw.len().saturating_sub(LOG_TAIL_BYTES);
    String::from_utf8_lossy(&raw[start..]).into_owned()
}

/// Whether the engine unit is running right now (`systemctl is-active`).
/// Read-only: no privilege, no sudoers entry.
pub fn engine_running() -> bool {
    run_timed(
        std::process::Command::new("systemctl").args(["is-active", "--quiet", UPDATE_UNIT]),
        START_TIMEOUT,
    )
    .map(|out| out.status.success())
    .unwrap_or(false)
}

/// Ask systemd to start the fixed engine unit.
pub fn start_engine() -> Result<(), String> {
    let output = run_timed(
        std::process::Command::new("sudo").args(["systemctl", "start", UPDATE_UNIT]),
        START_TIMEOUT,
    )
    .map_err(|e| {
        format!(
            "systemctl did not answer within {}s: {e}",
            START_TIMEOUT.as_secs()
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!(
                "systemctl start {UPDATE_UNIT} failed with {}",
                output.status
            )
        } else {
            stderr
        });
    }
    Ok(())
}

/// The engine itself: read the request, run the installer engine as root, and
/// record the outcome. This is the `--update-engine` mode; it runs inside the
/// systemd unit, not on the server runtime.
pub fn run_engine() -> Result<(), String> {
    let request = read_request().ok_or_else(|| {
        "no update request is queued (data/updates/request.json is missing)".to_string()
    })?;
    let manifest = validate_manifest(&request.manifest)?;
    let signature = request
        .signature
        .as_deref()
        .map(validate_signature)
        .transpose()?
        .flatten();
    let pubkey = request
        .pubkey
        .as_deref()
        .map(validate_pubkey)
        .transpose()?
        .flatten();
    let installer = installer_path()
        .ok_or_else(|| format!("the installer engine is not installed at {INSTALLER_PATH}"))?;

    let log = log_path();
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log)
        .map_err(|e| format!("cannot write {}: {e}", log.display()))?;

    let mut command = std::process::Command::new("bash");
    command
        .arg(&installer)
        // Non-interactive, plain text, and **no terminal relaunch**: the unit
        // has no TTY, and `HV_BOOTSTRAP=0` is the installer's documented
        // "already where you need to be" switch.
        .args(["--yes", "--cli"])
        .env("HV_BOOTSTRAP", "0")
        .env("HV_YES", "1")
        .env("HV_MODE", "cli")
        .env("HV_RELEASE_MANIFEST", &manifest)
        .stdout(log_file.try_clone().map_err(|e| e.to_string())?)
        .stderr(log_file);
    if let Some(signature) = &signature {
        command.env("HV_RELEASE_SIGNATURE", signature);
    }
    if let Some(pubkey) = &pubkey {
        command.env("HV_RELEASE_PUBKEY", pubkey);
    }

    let started = chrono::Utc::now();
    let mut echo = std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    let _ = writeln!(
        echo,
        "=== HomeVault update engine {} ({}) ===",
        started.to_rfc3339(),
        request.actor.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(echo, "manifest: {manifest}");

    let status = command.status();
    let exit = match status {
        Ok(status) if status.success() => EngineExit {
            status: "ok".to_string(),
            exit_code: status.code(),
            finished_at: chrono::Utc::now().to_rfc3339(),
        },
        Ok(status) => EngineExit {
            status: "failed".to_string(),
            exit_code: status.code(),
            finished_at: chrono::Utc::now().to_rfc3339(),
        },
        Err(e) => {
            let _ = writeln!(echo, "could not run the installer: {e}");
            EngineExit {
                status: "failed".to_string(),
                exit_code: None,
                finished_at: chrono::Utc::now().to_rfc3339(),
            }
        }
    };
    write_exit(&exit);
    // The request is consumed whatever the outcome: replaying it automatically
    // would retry a broken release in a loop.
    clear_request();
    if exit.status == "ok" {
        Ok(())
    } else {
        Err(format!(
            "the update engine exited with {}",
            exit.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "an error".to_string())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_validation() {
        assert_eq!(
            validate_manifest("https://releases.example.com/1.2.3/manifest.json").unwrap(),
            "https://releases.example.com/1.2.3/manifest.json"
        );
        assert_eq!(
            validate_manifest("/var/lib/homevault/updates/manifest.json").unwrap(),
            "/var/lib/homevault/updates/manifest.json"
        );
        for bad in [
            "",
            "http://releases.example.com/manifest.json",
            "file:///etc/passwd",
            "releases.example.com/manifest.json",
            "https://releases.example.com/a b",
            "/var/lib/../etc/passwd",
        ] {
            assert!(validate_manifest(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn test_signature_and_pubkey_validation() {
        assert_eq!(validate_signature("").unwrap(), None);
        assert_eq!(
            validate_signature("https://releases.example.com/manifest.json.sig").unwrap(),
            Some("https://releases.example.com/manifest.json.sig".to_string())
        );
        assert!(validate_signature("manifest.sig").is_err());

        assert_eq!(validate_pubkey("").unwrap(), None);
        assert_eq!(
            validate_pubkey("/usr/local/lib/homevault/release-key.pem").unwrap(),
            Some("/usr/local/lib/homevault/release-key.pem".to_string())
        );
        for bad in ["release-key.pem", "/etc/../etc/passwd", "/tmp/a b.pem"] {
            assert!(validate_pubkey(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn test_request_round_trip_shape() {
        let request = UpdateRequest {
            manifest: "https://releases.example.com/manifest.json".to_string(),
            signature: Some("https://releases.example.com/manifest.json.sig".to_string()),
            pubkey: None,
            actor: Some("admin".to_string()),
            requested_at: Some("2026-11-09T10:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: UpdateRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, request);

        // A minimal file (an operator's own script) still loads.
        let minimal: UpdateRequest =
            serde_json::from_str(r#"{"manifest":"https://example.com/m.json"}"#).unwrap();
        assert_eq!(minimal.signature, None);
        assert_eq!(minimal.actor, None);
    }

    #[test]
    fn test_engine_exit_shape() {
        let exit = EngineExit {
            status: "ok".to_string(),
            exit_code: Some(0),
            finished_at: "2026-11-09T10:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&exit).unwrap();
        let back: EngineExit = serde_json::from_str(&json).unwrap();
        assert_eq!(back, exit);
    }

    #[test]
    fn test_log_tail_is_bounded() {
        // A missing log is an empty string, never an error.
        assert_eq!(log_tail().len().min(LOG_TAIL_BYTES), log_tail().len());
    }
}
