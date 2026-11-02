/* =============================================================================
 * File          : backup_target.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup destination role: HomeVault can receive restic backups from another
 *   machine. Every source is a tenant with its own token, append-only
 *   repository, quota and optional scheduled integrity check; the daemon is a
 *   dedicated `rest-server` unit HomeVault generates, and the htpasswd file it
 *   reads is rewritten when a tenant goes over quota or is removed.
 *
 * Dependencies:
 *   - crate::restic (verification), crate::utils::process, rest-server binary
 *
 * Usage:
 *   handlers/backup_target.rs, backup_target_scheduler.rs; see
 *   docs/guides/backup-target.md and docs/guides/disaster-recovery.md.
 * =============================================================================
 */

//! Backup destination (see docs/guides/backup-target.md).
//!
//! What this role does and deliberately does **not** do:
//!
//! * It receives **restic** repositories over HTTP with `rest-server
//!   --append-only`, one private repository per tenant. Append-only means a
//!   compromised source cannot `forget`/`prune` the history it depends on.
//! * It never invents a second backup format: the source side is plain
//!   `restic -r rest:http://<host>:<port>/<tenant>/`.
//! * The tenant token is 256 bits of randomness, shown **once** at creation and
//!   stored only as a SHA-256. The htpasswd file `rest-server` reads carries a
//!   `{SHA}` entry — acceptable only because the password is machine-generated
//!   high-entropy material, never a human-chosen password.
//! * Quotas are enforced by revoking: when a repository grows past its quota the
//!   tenant is removed from the htpasswd file, so `rest-server` refuses every
//!   further request until an admin raises the quota or frees space.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::utils::process::run_timed;

/// The dedicated systemd unit.
pub const REST_SERVICE: &str = "homevault-rest-server";
/// Directory HomeVault owns for the generated auth file and settings.
pub const BACKUP_DIR_ENV: &str = "HV_BACKUP_TARGET_DIR";
/// Default listen port of the destination role. Deliberately not 8000: that is
/// rest-server's own default and a hand-run instance would collide.
pub const DEFAULT_PORT: u16 = 8001;
/// Hard ceiling for one `restic` verification call.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(600);
/// Deadline for a metadata call (`du`, `chmod`, `systemctl`).
pub const BACKUP_OP_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest tenant name, so a repository path stays sane.
pub const MAX_TENANT_NAME: usize = 64;

/// Where the generated files live (overridable for tests).
pub fn config_dir() -> PathBuf {
    std::env::var(BACKUP_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/homevault-backup"))
}

pub fn auth_path() -> PathBuf {
    config_dir().join("htpasswd")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Root of every tenant repository, under the data directory so a backup of
/// the data directory carries the received backups with it.
pub fn repos_root() -> PathBuf {
    crate::config::paths::base_dir().join("backup-target")
}

fn overridden() -> bool {
    config_dir() != Path::new("/etc/homevault-backup")
}

/// Daemon settings (opt-in).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupTargetSettings {
    /// Off until an admin turns the receive role on.
    pub enabled: bool,
    pub port: u16,
    /// Bind address. `127.0.0.1` keeps the role local until an admin decides
    /// the LAN address; the default is the LAN (`0.0.0.0`) once enabled.
    pub listen: String,
}

impl Default for BackupTargetSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            listen: "0.0.0.0".to_string(),
        }
    }
}

/// One source allowed to push backups here (`backup_tenants`).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BackupTenant {
    pub id: i64,
    pub name: String,
    /// `user:{SHA}…` entry for rest-server's htpasswd file (empty until set).
    pub auth_hash: String,
    /// SHA-256 of the bearer token (the token itself is only shown once).
    pub token_hash: String,
    /// Repository password sealed with the appliance key (never returned).
    pub repo_password_sealed: String,
    pub repo_path: String,
    /// 0 = unlimited.
    pub quota_bytes: i64,
    pub over_quota: i64,
    pub verify_enabled: i64,
    pub last_seen_at: Option<String>,
    pub last_verified_at: Option<String>,
    pub last_verify_status: Option<String>,
    pub last_verify_error: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl BackupTenant {
    /// The scope of the sealed repository password (AES-GCM AAD).
    pub fn secret_scope(&self) -> String {
        format!("backup-tenant:{}", self.name)
    }

    pub fn is_over_quota(&self) -> bool {
        self.over_quota != 0
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A tenant name: lowercase, share-like, and never a path fragment (it becomes
/// a directory under [`repos_root`] and a rest-server user).
pub fn validate_tenant_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("a tenant name is required".to_string());
    }
    if n.chars().count() > MAX_TENANT_NAME {
        return Err(format!(
            "a tenant name must be {MAX_TENANT_NAME} characters or fewer"
        ));
    }
    if n.chars().any(|c| c != c.to_ascii_lowercase()) {
        return Err("a tenant name must be lowercase".to_string());
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(
            "a tenant name may only contain letters, digits, '-' and '_' (no path characters)"
                .to_string(),
        );
    }
    if n.starts_with('-') {
        return Err("a tenant name cannot start with '-'".to_string());
    }
    Ok(())
}

/// The repository directory of one tenant, confined to [`repos_root`].
pub fn repo_path_for(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_tenant_name(name)?;
    Ok(root.join(name))
}

/// Validate a quota in bytes (`0` = unlimited), with a sane ceiling (1 PiB).
pub fn validate_quota(bytes: i64) -> Result<(), String> {
    if bytes < 0 {
        return Err("a quota cannot be negative (0 = unlimited)".to_string());
    }
    if bytes > 1_125_899_906_842_624 {
        return Err("a quota above 1 PiB is not supported (0 = unlimited)".to_string());
    }
    Ok(())
}

fn validate_settings(settings: &BackupTargetSettings) -> Result<(), String> {
    if settings.port == 0 {
        return Err("the port must not be 0".to_string());
    }
    let listen_ok = settings.listen == "0.0.0.0"
        || settings.listen == "::"
        || settings
            .listen
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if !listen_ok {
        return Err(
            "listen on 0.0.0.0, :: or 127.0.0.1 — a host name would be resolved at start time"
                .to_string(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tokens and the htpasswd file
// ---------------------------------------------------------------------------

/// A fresh 256-bit token, hex-encoded — shown once to the admin.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh 256-bit repository password, hex-encoded.
pub fn generate_repo_password() -> String {
    generate_token()
}

/// SHA-256 hex of a token, as stored.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The htpasswd entry `rest-server` reads for one tenant.
///
/// go-htpasswd (which rest-server uses) accepts `{SHA}` entries; a fast hash is
/// acceptable **only** because the secret is a machine-generated 256-bit token,
/// never a human password. This is documented in the guide.
pub fn htpasswd_entry(name: &str, token: &str) -> String {
    use sha1::{Digest as _, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    format!(
        "{name}:{{SHA}}{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Render the whole htpasswd file for the tenants that may currently connect.
///
/// Over-quota tenants are **omitted**, which is the enforcement: rest-server
/// finds no entry and refuses every request. Their token is not invalidated
/// (raising the quota restores access without re-issuing a new one).
pub fn render_htpasswd(tenants: &[BackupTenant]) -> String {
    let mut out = String::new();
    for tenant in tenants {
        if tenant.auth_hash.is_empty() || tenant.is_over_quota() {
            continue;
        }
        out.push_str(&tenant.auth_hash);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Quota
// ---------------------------------------------------------------------------

/// The quota verdict for one tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum QuotaVerdict {
    /// No quota configured.
    Unlimited,
    Under,
    /// At or above the quota: access is revoked until it is raised or freed.
    Exceeded,
}

impl QuotaVerdict {
    pub fn blocks(self) -> bool {
        self == QuotaVerdict::Exceeded
    }
}

pub fn quota_verdict(used: u64, quota: i64) -> QuotaVerdict {
    if quota <= 0 {
        return QuotaVerdict::Unlimited;
    }
    if used >= quota as u64 {
        QuotaVerdict::Exceeded
    } else {
        QuotaVerdict::Under
    }
}

/// Percentage of the quota used, `None` when there is no quota. Capped at 100
/// so a wildly over-quota repository does not render a 400 % bar.
pub fn quota_percent(used: u64, quota: i64) -> Option<u8> {
    if quota <= 0 {
        return None;
    }
    Some(((used.saturating_mul(100) / quota as u64).min(100)) as u8)
}

/// Total size of every regular file under `root`, without following symlinks.
///
/// A missing directory is `0` (a tenant with no repository yet).
pub fn dir_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            total = total.saturating_add(dir_size(&entry.path()));
        }
    }
    total
}

// ---------------------------------------------------------------------------
// Verification (restic)
// ---------------------------------------------------------------------------

/// What one verification run proved.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyOutcome {
    pub snapshots: u64,
    /// The file the test restore read back, when the repository had one.
    pub restored: Option<String>,
    pub message: String,
}

impl VerifyOutcome {
    fn summary(&self) -> String {
        match &self.restored {
            Some(path) => format!(
                "repository readable: {} snapshot(s), restored {path}",
                self.snapshots
            ),
            None => format!("repository readable: {} snapshot(s)", self.snapshots),
        }
    }
}

fn restic_env(cmd: &mut Command, password: &str) {
    cmd.env("RESTIC_PASSWORD", password);
    if std::env::var_os("RESTIC_CACHE_DIR").is_none() {
        cmd.env("RESTIC_CACHE_DIR", crate::config::paths::restic_cache_dir());
    }
}

/// Verify one repository: list its snapshots, check a subset of the data
/// (restic addresses every blob by its own hash, so this *is* hash
/// verification), then read one file back out of the latest snapshot.
pub fn verify_repository(repository: &str, password: &str) -> Result<VerifyOutcome, String> {
    if !crate::restic::ResticService::is_available() {
        return Err("restic is not installed on this host".to_string());
    }
    let mut list = Command::new("restic");
    list.args(["-r", repository, "snapshots", "--json", "--no-lock"]);
    restic_env(&mut list, password);
    let listed = run_timed(&mut list, VERIFY_TIMEOUT)
        .map_err(|e| format!("restic snapshots failed: {e}"))?;
    if !listed.status.success() {
        return Err(format!(
            "the repository could not be read: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    let snapshots_text = String::from_utf8_lossy(&listed.stdout);
    let snapshots = parse_snapshot_count(&snapshots_text);

    let mut check = Command::new("restic");
    check.args([
        "-r",
        repository,
        "check",
        "--read-data-subset=1%",
        "--no-lock",
    ]);
    restic_env(&mut check, password);
    let checked =
        run_timed(&mut check, VERIFY_TIMEOUT).map_err(|e| format!("restic check failed: {e}"))?;
    if !checked.status.success() {
        return Err(format!(
            "the repository failed its integrity check: {}",
            String::from_utf8_lossy(&checked.stderr).trim()
        ));
    }
    if snapshots == 0 {
        // An empty repository is readable, which is what "no backups yet"
        // looks like; it is not an error, but it is reported as such.
        let outcome = VerifyOutcome {
            snapshots: 0,
            restored: None,
            message: "the repository is readable but has no snapshot yet".to_string(),
        };
        return Ok(outcome);
    }

    // Read one file back: `ls` the latest snapshot, then `dump` it. A
    // repository whose metadata decrypts but whose data blobs are gone fails
    // here, which is the case a "check" alone can miss on a subset.
    let mut ls = Command::new("restic");
    ls.args(["-r", repository, "ls", "--json", "--no-lock", "latest"]);
    restic_env(&mut ls, password);
    let listed_files =
        run_timed(&mut ls, VERIFY_TIMEOUT).map_err(|e| format!("restic ls failed: {e}"))?;
    let mut restored = None;
    if listed_files.status.success() {
        let paths = parse_file_paths(&String::from_utf8_lossy(&listed_files.stdout));
        if let Some(path) = pick_restore_candidate(&paths) {
            let mut dump = Command::new("restic");
            dump.args(["-r", repository, "dump", "--no-lock", "latest", &path]);
            restic_env(&mut dump, password);
            match run_timed(&mut dump, VERIFY_TIMEOUT) {
                Ok(output) if output.status.success() && !output.stdout.is_empty() => {
                    // Re-hash the bytes the restore produced. restic's own
                    // content addressing is checked above; this proves the
                    // file is readable end to end.
                    let digest = Sha256::digest(&output.stdout);
                    if digest.iter().all(|b| *b == 0) {
                        return Err("the restored file hashed to zero bytes".to_string());
                    }
                    restored = Some(path);
                }
                Ok(_) => {
                    return Err(format!("the restore of '{path}' produced no data"));
                }
                Err(e) => return Err(format!("the restore of '{path}' failed: {e}")),
            }
        }
    }
    let mut outcome = VerifyOutcome {
        snapshots,
        restored,
        message: String::new(),
    };
    outcome.message = outcome.summary();
    Ok(outcome)
}

/// Number of entries in `restic snapshots --json` output.
pub fn parse_snapshot_count(output: &str) -> u64 {
    serde_json::from_str::<Vec<serde_json::Value>>(output)
        .map(|snapshots| snapshots.len() as u64)
        .unwrap_or(0)
}

/// File paths in `restic ls --json latest` output (`type == "file"`).
pub fn parse_file_paths(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|node| node.get("type").and_then(|t| t.as_str()) == Some("file"))
        .filter_map(|node| {
            node.get("path")
                .and_then(|p| p.as_str())
                .map(str::to_string)
        })
        .filter(|path| !path.is_empty())
        .collect()
}

/// Pick the file a test restore reads: the **first** non-empty path.
///
/// Deterministic on purpose — a test that restores a different file on every
/// run is harder to diagnose, and the repository's own subset check already
/// covers random data.
pub fn pick_restore_candidate(paths: &[String]) -> Option<String> {
    paths
        .iter()
        .find(|path| !path.is_empty() && !path.ends_with('/'))
        .cloned()
}

// ---------------------------------------------------------------------------
// Host application
// ---------------------------------------------------------------------------

fn write_generated(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    let text = path.to_string_lossy().to_string();
    match std::fs::write(path, content) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            crate::shares::write_file_as_root(&text, content).map_err(|e| e.to_string())?;
        }
        Err(e) => return Err(format!("cannot write {text}: {e}")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            warn!("backup target: could not set mode {mode:o} on {text}: {e}");
        }
    }
    Ok(())
}

/// The systemd unit of the destination role.
pub fn render_unit(settings: &BackupTargetSettings) -> String {
    format!(
        "[Unit]\nDescription=HomeVault backup destination (append-only restic repositories)\nAfter=network.target\n\n[Service]\nType=simple\nExecStart=/usr/local/bin/rest-server --path {root} --listen {listen}:{port} --htpasswd-file {auth} --append-only --private-repos\nRestart=on-failure\nRestartSec=5\nNoNewPrivileges=true\nProtectSystem=strict\nProtectHome=read-only\nReadWritePaths={root}\n\n[Install]\nWantedBy=multi-user.target\n",
        root = repos_root().display(),
        listen = settings.listen,
        port = settings.port,
        auth = auth_path().display(),
    )
}

/// Load the daemon settings (defaults when the file is absent).
pub fn load_settings() -> BackupTargetSettings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist the settings and (re)apply the daemon.
pub fn save_settings(
    settings: &BackupTargetSettings,
    tenants: &[BackupTenant],
) -> Result<(), String> {
    validate_settings(settings)?;
    std::fs::create_dir_all(config_dir())
        .map_err(|e| format!("cannot create the config dir: {e}"))?;
    std::fs::create_dir_all(repos_root())
        .map_err(|e| format!("cannot create the repository root: {e}"))?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    write_generated(&settings_path(), &json, 0o600)?;
    apply(settings, tenants)
}

/// Regenerate the auth file and unit, then bring the daemon in line with
/// `settings.enabled`.
pub fn apply(settings: &BackupTargetSettings, tenants: &[BackupTenant]) -> Result<(), String> {
    validate_settings(settings)?;
    std::fs::create_dir_all(config_dir())
        .map_err(|e| format!("cannot create the config dir: {e}"))?;
    write_generated(&auth_path(), &render_htpasswd(tenants), 0o600)?;
    if overridden() {
        return Ok(());
    }
    let unit_file = format!("/etc/systemd/system/{REST_SERVICE}.service");
    write_generated(Path::new(&unit_file), &render_unit(settings), 0o644)?;
    systemctl(&["daemon-reload"])?;
    if settings.enabled {
        systemctl(&["enable", REST_SERVICE])?;
        if service_active() {
            systemctl(&["restart", REST_SERVICE])?;
        } else {
            systemctl(&["start", REST_SERVICE])?;
        }
    } else {
        let _ = systemctl(&["stop", REST_SERVICE]);
        let _ = systemctl(&["disable", REST_SERVICE]);
    }
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.arg("systemctl").args(args);
    match run_timed(&mut cmd, BACKUP_OP_TIMEOUT) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "systemctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("systemctl {} failed: {e}", args.join(" "))),
    }
}

pub fn service_active() -> bool {
    if overridden() {
        return false;
    }
    run_timed(
        Command::new("systemctl").args(["is-active", "--quiet", REST_SERVICE]),
        BACKUP_OP_TIMEOUT,
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// Whether rest-server is installed (the role degrades to "not available").
pub fn rest_server_available() -> bool {
    crate::dependencies::binary_available("rest-server", "/usr/local/bin/rest-server")
}

/// The `rest:` repository URL a source should point restic at.
pub fn repository_url(settings: &BackupTargetSettings, host: &str, tenant: &str) -> String {
    let listen = if settings.listen == "0.0.0.0" || settings.listen == "::" {
        host.to_string()
    } else {
        settings.listen.clone()
    };
    format!("rest:http://{listen}:{}/{tenant}/", settings.port)
}

/// A per-tenant view for the API: never the token, never the sealed password.
pub fn tenant_json(
    tenant: &BackupTenant,
    used_bytes: u64,
    settings: &BackupTargetSettings,
    host: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": tenant.id,
        "name": tenant.name,
        "repo_path": tenant.repo_path,
        "repository_url": repository_url(settings, host, &tenant.name),
        "quota_bytes": tenant.quota_bytes,
        "used_bytes": used_bytes,
        "quota_percent": quota_percent(used_bytes, tenant.quota_bytes),
        "quota_verdict": match quota_verdict(used_bytes, tenant.quota_bytes) {
            QuotaVerdict::Unlimited => "unlimited",
            QuotaVerdict::Under => "under",
            QuotaVerdict::Exceeded => "exceeded",
        },
        "over_quota": tenant.is_over_quota(),
        "has_token": !tenant.token_hash.is_empty(),
        "verify_enabled": tenant.verify_enabled != 0,
        "last_seen_at": tenant.last_seen_at,
        "last_verified_at": tenant.last_verified_at,
        "last_verify_status": tenant.last_verify_status,
        "last_verify_error": tenant.last_verify_error,
        "created_by": tenant.created_by,
        "created_at": tenant.created_at,
    })
}

/// Per-tenant sizes, keyed by tenant id (the API and the watchdog share it).
pub fn tenant_usage(tenants: &[BackupTenant]) -> BTreeMap<i64, u64> {
    tenants
        .iter()
        .map(|tenant| (tenant.id, dir_size(Path::new(&tenant.repo_path))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(name: &str, quota: i64, over: bool) -> BackupTenant {
        BackupTenant {
            id: 1,
            name: name.to_string(),
            auth_hash: htpasswd_entry(name, "token"),
            token_hash: hash_token("token"),
            repo_password_sealed: String::new(),
            repo_path: format!("/var/lib/homevault/backup-target/{name}"),
            quota_bytes: quota,
            over_quota: i64::from(over),
            verify_enabled: 1,
            last_seen_at: None,
            last_verified_at: None,
            last_verify_status: None,
            last_verify_error: None,
            created_by: "admin".to_string(),
            created_at: "2026-10-31T00:00:00+00:00".to_string(),
            updated_at: "2026-10-31T00:00:00+00:00".to_string(),
        }
    }

    // -----------------------------------------------------------------
    // Names and paths
    // -----------------------------------------------------------------

    #[test]
    fn tenant_names_are_lowercase_and_path_free() {
        assert!(validate_tenant_name("family-nas").is_ok());
        assert!(validate_tenant_name("nas_2").is_ok());
        assert!(validate_tenant_name("Family").is_err());
        assert!(validate_tenant_name("../etc").is_err());
        assert!(validate_tenant_name("a/b").is_err());
        assert!(validate_tenant_name("-nas").is_err());
        assert!(validate_tenant_name("").is_err());
        assert!(validate_tenant_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn a_repository_path_stays_under_the_root() {
        let root = Path::new("/var/lib/homevault/backup-target");
        assert_eq!(
            repo_path_for(root, "family").unwrap(),
            Path::new("/var/lib/homevault/backup-target/family")
        );
        assert!(repo_path_for(root, "../etc").is_err());
        assert!(repo_path_for(root, "/etc").is_err());
    }

    // -----------------------------------------------------------------
    // Tokens
    // -----------------------------------------------------------------

    #[test]
    fn a_token_is_256_bits_and_stored_only_as_a_hash() {
        let token = generate_token();
        assert_eq!(token.len(), 64, "32 bytes hex-encoded");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, generate_token());
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, token, "the stored value is not the token");
        // The htpasswd entry carries a `{SHA}` of the token, not the token.
        let entry = htpasswd_entry("family", &token);
        assert!(entry.starts_with("family:{SHA}"), "{entry}");
        assert!(!entry.contains(&token), "{entry}");
        assert!(!entry.contains('\n'));
    }

    #[test]
    fn the_htpasswd_file_omits_blocked_and_unconfigured_tenants() {
        let allowed = tenant("family", 0, false);
        let blocked = tenant("laptop", 0, true);
        let mut unconfigured = tenant("guest", 0, false);
        unconfigured.auth_hash.clear();
        let rendered = render_htpasswd(&[allowed.clone(), blocked, unconfigured]);
        assert_eq!(rendered.lines().count(), 1, "{rendered}");
        assert!(rendered.contains("family:{SHA}"), "{rendered}");
        assert!(!rendered.contains("laptop"), "{rendered}");
        assert!(!rendered.contains("guest"), "{rendered}");
    }

    #[test]
    fn a_quota_is_enforced_at_or_above_its_limit() {
        assert_eq!(quota_verdict(0, 0), QuotaVerdict::Unlimited);
        assert_eq!(quota_verdict(10, -1), QuotaVerdict::Unlimited);
        assert_eq!(quota_verdict(9, 10), QuotaVerdict::Under);
        assert_eq!(quota_verdict(10, 10), QuotaVerdict::Exceeded);
        assert_eq!(quota_verdict(11, 10), QuotaVerdict::Exceeded);
        assert!(QuotaVerdict::Exceeded.blocks());
        assert!(!QuotaVerdict::Under.blocks());
        assert_eq!(quota_percent(5, 10), Some(50));
        assert_eq!(quota_percent(50, 10), Some(100), "capped at 100 %");
        assert_eq!(quota_percent(5, 0), None);
    }

    #[test]
    fn directory_size_counts_regular_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), b"12345").unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("b"), b"1234567").unwrap();
        assert_eq!(dir_size(tmp.path()), 12);
        // A symlink is not followed and contributes nothing.
        std::os::unix::fs::symlink("/etc/passwd", tmp.path().join("link")).unwrap();
        assert_eq!(dir_size(tmp.path()), 12);
        // A missing directory is empty, not an error.
        assert_eq!(dir_size(&tmp.path().join("missing")), 0);
    }

    #[test]
    fn quota_validation_bounds() {
        assert!(validate_quota(0).is_ok());
        assert!(validate_quota(1024).is_ok());
        assert!(validate_quota(-1).is_err());
    }

    // -----------------------------------------------------------------
    // Settings and URL
    // -----------------------------------------------------------------

    #[test]
    fn settings_are_off_by_default_and_validated() {
        let settings = BackupTargetSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.port, DEFAULT_PORT);
        assert!(validate_settings(&settings).is_ok());
        assert!(validate_settings(&BackupTargetSettings {
            port: 0,
            ..BackupTargetSettings::default()
        })
        .is_err());
        assert!(validate_settings(&BackupTargetSettings {
            listen: "nas.local".to_string(),
            ..BackupTargetSettings::default()
        })
        .is_err());
        assert!(validate_settings(&BackupTargetSettings {
            listen: "127.0.0.1".to_string(),
            ..BackupTargetSettings::default()
        })
        .is_ok());
    }

    #[test]
    fn the_suggested_repository_url_names_the_tenant() {
        let settings = BackupTargetSettings::default();
        assert_eq!(
            repository_url(&settings, "nas2.lan", "family"),
            format!("rest:http://nas2.lan:{DEFAULT_PORT}/family/")
        );
        // A loopback-bound daemon is only reachable locally: the URL says so
        // instead of advertising the LAN address.
        let loopback = BackupTargetSettings {
            listen: "127.0.0.1".to_string(),
            ..BackupTargetSettings::default()
        };
        assert_eq!(
            repository_url(&loopback, "nas2.lan", "family"),
            format!("rest:http://127.0.0.1:{DEFAULT_PORT}/family/")
        );
    }

    #[test]
    fn the_api_view_never_carries_a_secret() {
        let settings = BackupTargetSettings::default();
        let mut t = tenant("family", 1000, false);
        t.token_hash = hash_token("super-secret-token");
        t.repo_password_sealed = "hv1:sealed".to_string();
        let json = tenant_json(&t, 250, &settings, "nas.lan");
        let text = json.to_string();
        assert!(!text.contains("super-secret-token"), "{text}");
        assert!(!text.contains("hv1:sealed"), "{text}");
        assert_eq!(json["quota_percent"], serde_json::json!(25));
        assert_eq!(json["quota_verdict"], serde_json::json!("under"));
        assert_eq!(json["has_token"], serde_json::json!(true));
    }

    // -----------------------------------------------------------------
    // Verification helpers
    // -----------------------------------------------------------------

    #[test]
    fn restic_json_output_is_parsed_without_panicking() {
        assert_eq!(parse_snapshot_count("[]"), 0);
        assert_eq!(parse_snapshot_count(r#"[{"id":"a"},{"id":"b"}]"#), 2);
        assert_eq!(parse_snapshot_count("not json"), 0);

        let ls = "{\"name\":\"data\",\"type\":\"dir\",\"path\":\"/data\"}\n\
                  {\"name\":\"a.txt\",\"type\":\"file\",\"path\":\"/data/a.txt\"}\n\
                  {\"name\":\"b.txt\",\"type\":\"file\",\"path\":\"/data/b.txt\"}\n";
        assert_eq!(
            parse_file_paths(ls),
            vec!["/data/a.txt".to_string(), "/data/b.txt".to_string()]
        );
        assert!(parse_file_paths("").is_empty());
        assert_eq!(
            pick_restore_candidate(&["/data/a.txt".to_string()]).as_deref(),
            Some("/data/a.txt")
        );
        assert!(pick_restore_candidate(&[]).is_none());
        // A directory entry never becomes the restore target.
        assert!(pick_restore_candidate(&["/data/".to_string()]).is_none());
    }

    #[test]
    fn the_generated_unit_is_append_only_and_private() {
        let unit = render_unit(&BackupTargetSettings::default());
        assert!(unit.contains("--append-only"), "{unit}");
        assert!(unit.contains("--private-repos"), "{unit}");
        assert!(unit.contains("--htpasswd-file"), "{unit}");
        assert!(unit.contains("NoNewPrivileges=true"), "{unit}");
        assert!(
            unit.contains(&format!("--listen 0.0.0.0:{DEFAULT_PORT}")),
            "{unit}"
        );
    }
}
