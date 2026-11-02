/* =============================================================================
 * File          : iscsi.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   iSCSI target support (phase P3.3) on top of the kernel's LIO stack: file
 *   and block backstores, LUNs, targets, portals, initiator ACLs and optional
 *   per-initiator CHAP. Validation, the IQN/portal/LUN/CHAP rules and the
 *   `targetcli` argv are pure; only `apply` touches the host.
 *
 *   The feature is opt-in twice: an admin turns it on (JSON settings) **and**
 *   the host must provide the LIO kernel modules plus `targetcli`. When either
 *   is missing the screen hides the editor and says which one is missing.
 *
 * Dependencies:
 *   - serde (>= 1.0), chrono (>= 0.4), crate::utils::process
 *
 * Usage:
 *   handlers::iscsi exposes the endpoints; see docs/guides/iscsi.md.
 * =============================================================================
 */

//! iSCSI target management (see docs/guides/iscsi.md).
//!
//! Rules this module is built around:
//!
//! * **Metadata first, host second.** Creating, renaming or editing a backstore,
//!   target, LUN, portal or ACL only writes rows. Nothing reaches the kernel
//!   until an admin presses Apply, so a half-built export never becomes visible
//!   to an initiator by accident.
//! * **Deleting a record is not deleting data.** Forgetting a backstore removes
//!   its metadata (and its LUNs); the backing file stays on disk unless the
//!   caller explicitly asks for deletion *and* repeats the backstore name.
//! * **Only what HomeVault owns is removed.** Apply/teardown names the exact
//!   objects the metadata lists. It never runs `targetcli clearconfig`, which
//!   would delete a target an admin configured by hand.
//! * **No shell.** Every call is an argv vector; the IQN, portal address and
//!   backstore path are the only free-text values and each is validated before
//!   it can reach `targetcli`.

use std::path::Path;
use std::process::Command as ProcessCommand;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::storage::json::JsonStorage;
use crate::utils::process::run_timed;

/// Storage key → `{DATA_DIRECTORY}/data/iscsi.json`.
const STORE_KEY: &str = "iscsi";

/// Kernel modules the LIO stack needs, in load order.
pub const MODULES: &[&str] = &[
    "target_core_mod",
    "iscsi_target_mod",
    "target_core_file",
    "target_core_iblock",
];

/// Where the kernel exposes configfs, which is what `targetcli` writes into.
pub const CONFIGFS_TARGET_PATH: &str = "/sys/kernel/config/target";

/// Default portal: every interface, the IANA-assigned iSCSI port.
pub const DEFAULT_PORTAL_ADDRESS: &str = "0.0.0.0";
pub const DEFAULT_PORTAL_PORT: u16 = 3260;

/// Bounds. `MAX_LUN` follows the iSCSI spec (14-bit LUN field).
pub const MAX_BACKSTORE_NAME: usize = 64;
pub const MAX_IQN_LEN: usize = 223;
pub const MAX_LUN: i64 = 16_383;
pub const MAX_BACKSTORE_SIZE: i64 = 64 * 1024 * 1024 * 1024 * 1024;
pub const MAX_PORTALS_PER_TARGET: usize = 8;
pub const MAX_LUNS_PER_TARGET: usize = 32;
pub const MAX_ACLS_PER_TARGET: usize = 32;
pub const MAX_TARGETS: usize = 32;
pub const MAX_BACKSTORES: usize = 64;

/// CHAP bounds. RFC 7143 mandates 12–16 octets for the shared secret; the
/// userid is a short printable token.
pub const CHAP_SECRET_MIN: usize = 12;
pub const CHAP_SECRET_MAX: usize = 16;
pub const CHAP_USERID_MAX: usize = 64;

/// Deadline for one `targetcli` invocation.
pub const TARGETCLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Deadline for one `modprobe`.
pub const MODPROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What kind of storage a backstore exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackstoreKind {
    /// A regular file sized by HomeVault (`targetcli /backstores/fileio`).
    Fileio,
    /// An existing block device (`targetcli /backstores/block`).
    Block,
}

impl BackstoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackstoreKind::Fileio => "fileio",
            BackstoreKind::Block => "block",
        }
    }

    /// The boolean a `fileio` backstore is created with.
    pub fn is_file(self) -> bool {
        matches!(self, BackstoreKind::Fileio)
    }
}

/// The persisted feature switch (`{DATA_DIRECTORY}/data/iscsi.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IscsiSettings {
    pub enabled: bool,
    /// RFC 3339 stamp of the last successful apply. Compared against the
    /// newest row timestamp to decide whether metadata is still pending.
    #[serde(default)]
    pub applied_at: Option<String>,
    pub updated_at: String,
}

/// The AES-GCM scope a CHAP secret is sealed under. It names the target by its
/// IQN (not by its row id: a new target has no id yet when its secret is
/// sealed) and the initiator, so a blob lifted from one export cannot be
/// opened under another. The IQN cannot be changed in place, so it is stable
/// for the life of a target.
pub fn chap_scope(target_iqn: &str, initiator_iqn: &str) -> String {
    format!(
        "iscsi-chap:{}:{}",
        target_iqn.trim().to_ascii_lowercase(),
        initiator_iqn.trim().to_ascii_lowercase()
    )
}

/// One exported backstore.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IscsiBackstore {
    pub id: i64,
    pub name: String,
    /// `fileio` | `block`
    pub kind: String,
    pub path: String,
    pub size_bytes: i64,
    pub readonly: bool,
    pub created_at: String,
    pub updated_at: String,
    /// How many LUNs point at this backstore. Computed by every repository
    /// read; a backstore in use cannot be forgotten.
    pub lun_count: i64,
}

impl IscsiBackstore {
    pub fn kind(&self) -> BackstoreKind {
        if self.kind == "block" {
            BackstoreKind::Block
        } else {
            BackstoreKind::Fileio
        }
    }
}

/// One portal a target listens on.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IscsiPortal {
    pub id: i64,
    pub target_id: i64,
    pub address: String,
    pub port: i64,
}

/// One LUN of a target. `backstore_name` comes from a join and is what the UI
/// shows; the kernel object is named after it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IscsiLun {
    pub id: i64,
    pub target_id: i64,
    pub lun: i64,
    pub backstore_id: i64,
    #[sqlx(default)]
    pub backstore_name: String,
}

/// One initiator allowed to log in to a target.
#[derive(Debug, Clone, Serialize)]
pub struct IscsiAcl {
    pub id: i64,
    pub target_id: i64,
    pub initiator_iqn: String,
    /// CHAP userid, or `None` when this initiator uses no authentication.
    pub userid: Option<String>,
    /// Whether a CHAP secret is stored. The secret itself is never returned.
    pub has_secret: bool,
}

/// A target with everything it owns. One aggregate because the editor saves
/// the whole export at once.
#[derive(Debug, Clone, Serialize)]
pub struct IscsiTarget {
    pub id: i64,
    pub iqn: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub portals: Vec<IscsiPortal>,
    pub luns: Vec<IscsiLun>,
    pub acls: Vec<IscsiAcl>,
}

/// What the host can actually do.
#[derive(Debug, Clone, Serialize)]
pub struct IscsiCapabilities {
    /// Every LIO module is loadable or already loaded.
    pub kernel_available: bool,
    /// `configfs` is mounted where `targetcli` expects it.
    pub configfs_available: bool,
    /// The `targetcli` binary is on the box.
    pub targetcli_available: bool,
    /// Modules present right now.
    pub modules_loaded: Vec<String>,
    /// Modules that are missing and would have to be loaded.
    pub modules_missing: Vec<String>,
    /// True only when all three are true: the screen may show the editor.
    pub usable: bool,
}

/// The whole host-side answer.
pub fn capabilities() -> IscsiCapabilities {
    let modules_loaded: Vec<String> = MODULES
        .iter()
        .filter(|m| Path::new(&format!("/sys/module/{m}")).exists())
        .map(|m| m.to_string())
        .collect();
    let modules_missing: Vec<String> = MODULES
        .iter()
        .filter(|m| !Path::new(&format!("/sys/module/{m}")).exists())
        .map(|m| m.to_string())
        .collect();
    let configfs_available = Path::new(CONFIGFS_TARGET_PATH).exists();
    let targetcli_available =
        crate::dependencies::binary_available("targetcli", "/usr/sbin/targetcli");
    // A module list is "available" when the tree provides them: either loaded
    // already, or the module file exists so `modprobe` can load it.
    let kernel_available = MODULES
        .iter()
        .all(|m| Path::new(&format!("/sys/module/{m}")).exists() || module_file_exists(m));
    IscsiCapabilities {
        kernel_available,
        configfs_available,
        targetcli_available,
        modules_loaded,
        modules_missing,
        usable: kernel_available && configfs_available && targetcli_available,
    }
}

/// Whether the kernel ships a loadable module file for `name`. The search is
/// deliberately shallow — the kernel version directory differs per host.
fn module_file_exists(name: &str) -> bool {
    let modules_root = Path::new("/lib/modules");
    let Ok(entries) = std::fs::read_dir(modules_root) else {
        return false;
    };
    for entry in entries.flatten() {
        let kernels = entry.path().join("kernel");
        if find_module_file(&kernels, name) {
            return true;
        }
    }
    false
}

fn find_module_file(dir: &Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if find_module_file(&path, name) {
                return true;
            }
        } else if let Some(file) = path.file_name().and_then(|f| f.to_str()) {
            if file == format!("{name}.ko")
                || file == format!("{name}.ko.xz")
                || file == format!("{name}.ko.zst")
                || file == format!("{name}.ko.gz")
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A backstore name is an object name in `targetcli` (`/backstores/fileio/<name>`)
/// and appears in command argv, so it is kept to one conservative charset.
pub fn valid_backstore_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_BACKSTORE_NAME {
        return false;
    }
    if name.starts_with('.') || name.starts_with('-') {
        return false;
    }
    if name == "default" || name == "global" {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// An initiator/target IQN. RFC 3720 allows `iqn.` (iSCSI qualified), `eui.`
/// and `naa.`; the character set is deliberately narrower than the grammar
/// because the value ends up in a `targetcli` argv and a directory name.
pub fn valid_iqn(iqn: &str) -> bool {
    let iqn = iqn.trim();
    if iqn.len() < 8 || iqn.len() > MAX_IQN_LEN {
        return false;
    }
    let lower = iqn.to_ascii_lowercase();
    let known_prefix =
        lower.starts_with("iqn.") || lower.starts_with("eui.") || lower.starts_with("naa.");
    if !known_prefix {
        return false;
    }
    if iqn.contains("..") || iqn.contains(' ') {
        return false;
    }
    // An `iqn.` must carry the reverse-domain part (`iqn.2026-11.com.example`).
    if let Some(rest) = lower.strip_prefix("iqn.") {
        if !rest.contains('.') || rest.len() < 10 {
            return false;
        }
    }
    iqn.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ':' || c == '-')
}

/// A portal address is an IP literal. Host names would need DNS at apply time
/// and can change under the target, so they are refused.
pub fn valid_portal_address(address: &str) -> bool {
    address.trim().parse::<std::net::IpAddr>().is_ok()
}

pub fn valid_portal_port(port: i64) -> bool {
    (1..=65535).contains(&port)
}

pub fn valid_lun(lun: i64) -> bool {
    (0..=MAX_LUN).contains(&lun)
}

/// CHAP credentials, or `None` when the initiator uses no authentication.
pub fn validate_chap(userid: Option<&str>, secret: Option<&str>) -> Result<(), String> {
    match (userid, secret) {
        (None, None) => Ok(()),
        (Some(userid), Some(secret)) => {
            let userid = userid.trim();
            if userid.is_empty() || userid.len() > CHAP_USERID_MAX {
                return Err("The CHAP user name must be 1–64 characters".to_string());
            }
            if !userid.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                return Err("The CHAP user name must be printable ASCII".to_string());
            }
            let len = secret.len();
            if !(CHAP_SECRET_MIN..=CHAP_SECRET_MAX).contains(&len) {
                return Err("The CHAP secret must be 12–16 characters (RFC 7143)".to_string());
            }
            if !secret.chars().all(|c| c.is_ascii_graphic()) {
                return Err("The CHAP secret must be printable ASCII".to_string());
            }
            Ok(())
        }
        _ => Err("A CHAP user name and secret must be given together".to_string()),
    }
}

/// Validate a whole target the way the editor sends it.
pub fn validate_target(
    iqn: &str,
    portals: &[(String, i64)],
    luns: &[i64],
    acls: &[(String, Option<String>, Option<String>)],
) -> Result<(), String> {
    if !valid_iqn(iqn) {
        return Err(format!("'{iqn}' is not a valid iSCSI qualified name"));
    }
    if portals.is_empty() {
        return Err("A target needs at least one portal".to_string());
    }
    if portals.len() > MAX_PORTALS_PER_TARGET {
        return Err(format!(
            "A target cannot have more than {MAX_PORTALS_PER_TARGET} portals"
        ));
    }
    let mut seen_portals = Vec::new();
    for (address, port) in portals {
        if !valid_portal_address(address) {
            return Err(format!("'{address}' is not an IP address"));
        }
        if !valid_portal_port(*port) {
            return Err("A portal port must be between 1 and 65535".to_string());
        }
        let key = (address.trim().to_string(), *port);
        if seen_portals.contains(&key) {
            return Err(format!("The portal {}:{} is listed twice", key.0, key.1));
        }
        seen_portals.push(key);
    }
    if luns.len() > MAX_LUNS_PER_TARGET {
        return Err(format!(
            "A target cannot have more than {MAX_LUNS_PER_TARGET} LUNs"
        ));
    }
    let mut seen_luns = Vec::new();
    for lun in luns {
        if !valid_lun(*lun) {
            return Err(format!("LUN {lun} is outside 0–{MAX_LUN}"));
        }
        if seen_luns.contains(lun) {
            return Err(format!("LUN {lun} is used twice on this target"));
        }
        seen_luns.push(*lun);
    }
    if acls.len() > MAX_ACLS_PER_TARGET {
        return Err(format!(
            "A target cannot have more than {MAX_ACLS_PER_TARGET} initiators"
        ));
    }
    let mut seen_acls = Vec::new();
    for (initiator, userid, secret) in acls {
        if !valid_iqn(initiator) {
            return Err(format!("'{initiator}' is not a valid initiator name"));
        }
        let key = initiator.trim().to_ascii_lowercase();
        if seen_acls.contains(&key) {
            return Err(format!("The initiator '{initiator}' is listed twice"));
        }
        seen_acls.push(key);
        validate_chap(userid.as_deref(), secret.as_deref())?;
    }
    Ok(())
}

/// A backstore path must be absolute and free of traversal. Confinement to the
/// appliance's allowed roots is the handler's job (it has the `PathGuard`).
pub fn validate_backstore_path(path: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("A backstore needs a path".to_string());
    }
    if trimmed.contains('\0') {
        return Err("The path contains an invalid character".to_string());
    }
    if !trimmed.starts_with('/') {
        return Err("The backstore path must be absolute".to_string());
    }
    if trimmed.split('/').any(|segment| segment == "..") {
        return Err("The backstore path must not contain '..'".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command generation
// ---------------------------------------------------------------------------

/// One `targetcli` (or `modprobe`) invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IscsiCommand {
    pub argv: Vec<String>,
}

impl IscsiCommand {
    fn new(parts: &[&str]) -> Self {
        Self {
            argv: parts.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn with_sudo(self) -> Self {
        let mut argv = vec!["sudo".to_string()];
        argv.extend(self.argv);
        Self { argv }
    }

    /// Rendering for the UI and the event log. Never contains a CHAP secret:
    /// that argument is passed through `targetcli`'s `set` with the value
    /// redacted before the command is described (see [`redacted`]).
    pub fn describe(&self) -> String {
        redact(&self.argv.join(" "))
    }
}

/// Replace a CHAP secret's value in a rendered command. `targetcli` takes
/// `set auth password=<secret>`, so the described form keeps the key but not
/// the value.
fn redact(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for token in line.split(' ') {
        if let Some((key, _)) = token.split_once('=') {
            if key == "password" || key == "userid_secret" {
                out.push_str(key);
                out.push_str("=***");
                out.push(' ');
                continue;
            }
        }
        out.push_str(token);
        out.push(' ');
    }
    out.trim_end().to_string()
}

/// `modprobe` the modules that are missing. Empty when all are present.
pub fn module_commands(missing: &[String]) -> Vec<IscsiCommand> {
    missing
        .iter()
        .map(|module| IscsiCommand::new(&["modprobe", module]).with_sudo())
        .collect()
}

/// Create (or update) one backstore. `replace` makes a second apply idempotent:
/// `targetcli` answers "already exists" to a plain create.
pub fn backstore_create_command(backstore: &IscsiBackstore) -> IscsiCommand {
    match backstore.kind() {
        BackstoreKind::Fileio => IscsiCommand::new(&[
            "targetcli",
            "/backstores/fileio",
            "create",
            &backstore.name,
            &backstore.path,
            &backstore.size_bytes.to_string(),
            if backstore.readonly {
                "readonly=1"
            } else {
                "readonly=0"
            },
        ])
        .with_sudo(),
        BackstoreKind::Block => IscsiCommand::new(&[
            "targetcli",
            "/backstores/block",
            "create",
            &backstore.name,
            &format!("dev={}", backstore.path),
            if backstore.readonly {
                "readonly=1"
            } else {
                "readonly=0"
            },
        ])
        .with_sudo(),
    }
}

/// Forget one backstore object. The backing file is **not** deleted — that is
/// a separate, explicitly confirmed action ([`delete_backing_file`]).
pub fn backstore_delete_command(backstore: &IscsiBackstore) -> IscsiCommand {
    let group = match backstore.kind() {
        BackstoreKind::Fileio => "/backstores/fileio",
        BackstoreKind::Block => "/backstores/block",
    };
    IscsiCommand::new(&["targetcli", group, "delete", &backstore.name]).with_sudo()
}

/// Every command that brings one target up from its metadata: the target
/// object, its portals, its LUNs (by backstore name), its ACLs and CHAP, and
/// the enable flag.
///
/// Portals are set before LUNs and ACLs because a target with no portal is
/// never reachable, and `enable` is last so an initiator cannot log in to a
/// half-built target.
pub fn target_apply_commands(
    target: &IscsiTarget,
    backstores: &[IscsiBackstore],
    chap_secrets: &[(String, String, String)],
) -> Vec<IscsiCommand> {
    let iqn = &target.iqn;
    let base = format!("/iscsi/{iqn}");
    let tpg = format!("{base}/tpg1");
    let mut commands = vec![IscsiCommand::new(&["targetcli", "/iscsi", "create", iqn]).with_sudo()];

    for portal in &target.portals {
        commands.push(
            IscsiCommand::new(&[
                "targetcli",
                &format!("{tpg}/portals"),
                "create",
                &portal.address,
                &portal.port.to_string(),
            ])
            .with_sudo(),
        );
    }

    for lun in &target.luns {
        // A LUN whose backstore vanished is skipped rather than pointed at a
        // name that does not exist, which `targetcli` would refuse.
        let Some(backstore) = backstores.iter().find(|b| b.id == lun.backstore_id) else {
            continue;
        };
        commands.push(
            IscsiCommand::new(&[
                "targetcli",
                &format!("{tpg}/luns"),
                "create",
                &format!(
                    "/backstores/{}/{}",
                    backstore.kind().as_str(),
                    backstore.name
                ),
                &lun.lun.to_string(),
            ])
            .with_sudo(),
        );
    }

    for acl in &target.acls {
        let acl_path = format!("{tpg}/acls");
        commands.push(
            IscsiCommand::new(&["targetcli", &acl_path, "create", &acl.initiator_iqn]).with_sudo(),
        );
        if let Some(userid) = &acl.userid {
            commands.push(
                IscsiCommand::new(&[
                    "targetcli",
                    &format!("{acl_path}/{}", acl.initiator_iqn),
                    "set",
                    "auth",
                    &format!("userid={userid}"),
                ])
                .with_sudo(),
            );
            if let Some((_, _, secret)) = chap_secrets.iter().find(|(iqn, initiator, _)| {
                iqn.eq_ignore_ascii_case(target.iqn.trim())
                    && initiator.eq_ignore_ascii_case(acl.initiator_iqn.trim())
            }) {
                commands.push(
                    IscsiCommand::new(&[
                        "targetcli",
                        &format!("{acl_path}/{}", acl.initiator_iqn),
                        "set",
                        "auth",
                        &format!("password={secret}"),
                    ])
                    .with_sudo(),
                );
            }
        }
    }

    // `demo_mode_write_protect` is the LIO default and silently makes every
    // LUN read-only; an explicit `0` is what makes a read/write export work.
    commands.push(
        IscsiCommand::new(&[
            "targetcli",
            &tpg,
            "set",
            "attribute",
            "demo_mode_write_protect=0",
        ])
        .with_sudo(),
    );
    // `enable` last: the target is only reachable once every portal, LUN, ACL
    // and attribute is in place, so an initiator cannot log in to a half-built
    // export.
    commands.push(IscsiCommand::new(&["targetcli", &tpg, "enable"]).with_sudo());
    commands
}

/// Tear one target down. Deleting the `/iscsi/<iqn>` object removes its
/// portals, LUNs and ACLs with it, so one command is enough — and it names the
/// exact IQN, so no hand-made target is touched.
pub fn target_teardown_command(iqn: &str) -> IscsiCommand {
    IscsiCommand::new(&["targetcli", "/iscsi", "delete", iqn]).with_sudo()
}

/// [`target_teardown_command`], executed — the path the handlers take when an
/// admin forgets a target. A host without the LIO stack has nothing to remove
/// and reports success; otherwise a failure is returned, never swallowed, so
/// the row is not deleted while the export stays live.
pub fn unexport_target_blocking(iqn: &str) -> Result<(), String> {
    if !capabilities().usable {
        return Ok(());
    }
    run(&target_teardown_command(iqn), TARGETCLI_TIMEOUT)
}

/// Execute one [`IscsiCommand`] with the standard deadline. Exposed so the
/// backstore-delete handler and any future caller run the same code path as
/// `apply_sync`.
pub fn run_command_blocking(command: &IscsiCommand, deadline: Duration) -> Result<(), String> {
    run(command, deadline)
}

/// Remove the backing file of a `fileio` backstore. Only reached when the
/// caller explicitly asked for data deletion with the name typed back.
pub fn delete_backing_file(path: &str) -> Result<(), String> {
    let file = Path::new(path);
    if !file.is_absolute() {
        return Err("Refusing to delete a relative path".to_string());
    }
    if !file.exists() {
        // Already gone: the metadata deletion still succeeds.
        return Ok(());
    }
    let metadata = std::fs::metadata(file).map_err(|e| format!("Could not inspect {path}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    std::fs::remove_file(file).map_err(|e| format!("Could not delete {path}: {e}"))
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

/// What one apply pass did.
#[derive(Debug, Clone, Serialize)]
pub struct IscsiApplyReport {
    pub applied: bool,
    pub reason: Option<String>,
    /// Machine-readable reason, so the UI localises it instead of echoing an
    /// English sentence: `off`, `no_targetcli`, `no_kernel`, `empty`.
    pub reason_code: Option<&'static str>,
    pub commands: Vec<String>,
    pub failed: Option<String>,
}

fn run(command: &IscsiCommand, deadline: Duration) -> Result<(), String> {
    let mut cmd = ProcessCommand::new(&command.argv[0]);
    cmd.args(&command.argv[1..]);
    match run_timed(&mut cmd, deadline) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "`{}` failed: {}",
            command.describe(),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("`{}` did not finish: {e}", command.describe())),
    }
}

/// Apply the whole export configuration to the host. Blocking; call it on the
/// blocking pool.
///
/// `chap_secrets` carries the opened secrets as `(target IQN, initiator IQN,
/// secret)`; they are used to build the commands and never appear in the
/// report.
pub fn apply_sync(
    settings: &IscsiSettings,
    backstores: &[IscsiBackstore],
    targets: &[IscsiTarget],
    chap_secrets: &[(String, String, String)],
) -> IscsiApplyReport {
    let caps = capabilities();
    if !caps.targetcli_available {
        return IscsiApplyReport {
            applied: false,
            reason: Some("The `targetcli` tool is not installed".to_string()),
            reason_code: Some("no_targetcli"),
            commands: Vec::new(),
            failed: None,
        };
    }
    if !caps.kernel_available || !caps.configfs_available {
        return IscsiApplyReport {
            applied: false,
            reason: Some("The iSCSI kernel modules are not available on this host".to_string()),
            reason_code: Some("no_kernel"),
            commands: Vec::new(),
            failed: None,
        };
    }
    let enabled_targets: Vec<&IscsiTarget> = targets.iter().filter(|t| t.enabled).collect();
    if !settings.enabled || enabled_targets.is_empty() {
        // Tear down only what the metadata owns; a no-op when nothing is saved.
        // Every listed target goes, not just the enabled ones: an export that
        // was disabled without an apply may still be live in the kernel.
        let mut commands = Vec::new();
        for target in targets {
            commands.push(target_teardown_command(&target.iqn));
        }
        for backstore in backstores {
            commands.push(backstore_delete_command(backstore));
        }
        let mut executed = Vec::new();
        for command in &commands {
            executed.push(command.describe());
            // A missing object is the normal case for a target that was never
            // applied; a failure here must not abort the teardown of the rest.
            let _ = run(command, TARGETCLI_TIMEOUT);
        }
        return IscsiApplyReport {
            applied: false,
            reason: Some(if settings.enabled {
                "No target is enabled".to_string()
            } else {
                "iSCSI is turned off".to_string()
            }),
            reason_code: Some(if settings.enabled { "empty" } else { "off" }),
            commands: executed,
            failed: None,
        };
    }

    let mut executed = Vec::new();
    for command in module_commands(&caps.modules_missing) {
        executed.push(command.describe());
        if let Err(e) = run(&command, MODPROBE_TIMEOUT) {
            return IscsiApplyReport {
                applied: false,
                reason: Some("The iSCSI kernel modules could not be loaded".to_string()),
                reason_code: Some("no_kernel"),
                commands: executed,
                failed: Some(e),
            };
        }
    }

    // Apply always rebuilds from the metadata. An export edited in place (a
    // LUN or a portal removed) or turned off must stop being live, and a
    // second apply must not fail on "already exists" — so every object the
    // metadata names is forgotten first. Only those names are touched: a
    // target configured by hand is left alone. A missing object is the normal
    // case on the first apply, so a failure here is never fatal.
    for target in targets {
        let command = target_teardown_command(&target.iqn);
        executed.push(command.describe());
        let _ = run(&command, TARGETCLI_TIMEOUT);
    }
    for backstore in backstores {
        let command = backstore_delete_command(backstore);
        executed.push(command.describe());
        let _ = run(&command, TARGETCLI_TIMEOUT);
    }

    for backstore in backstores {
        let command = backstore_create_command(backstore);
        executed.push(command.describe());
        if let Err(e) = run(&command, TARGETCLI_TIMEOUT) {
            return IscsiApplyReport {
                applied: false,
                reason: Some("The configuration could not be applied".to_string()),
                reason_code: None,
                commands: executed,
                failed: Some(e),
            };
        }
    }
    for target in &enabled_targets {
        for command in target_apply_commands(target, backstores, chap_secrets) {
            executed.push(command.describe());
            if let Err(e) = run(&command, TARGETCLI_TIMEOUT) {
                return IscsiApplyReport {
                    applied: false,
                    reason: Some("The configuration could not be applied".to_string()),
                    reason_code: None,
                    commands: executed,
                    failed: Some(e),
                };
            }
        }
    }
    IscsiApplyReport {
        applied: true,
        reason: None,
        reason_code: None,
        commands: executed,
        failed: None,
    }
}

/// Load the saved switch, or the default (off).
pub fn load() -> IscsiSettings {
    match JsonStorage::new(data_directory()).load::<IscsiSettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => IscsiSettings::default(),
        Err(e) => {
            tracing::warn!("could not read the iSCSI settings: {e}");
            IscsiSettings::default()
        }
    }
}

/// Persist the switch (atomic write).
pub fn save(settings: &IscsiSettings) -> Result<(), String> {
    JsonStorage::new(data_directory())
        .save(STORE_KEY, settings)
        .map_err(|e| format!("could not save the iSCSI settings: {e}"))
}

fn data_directory() -> String {
    std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string())
}

/// The default switch, stamped now.
pub fn new_settings(enabled: bool) -> IscsiSettings {
    IscsiSettings {
        enabled,
        applied_at: None,
        updated_at: Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chap_scope_is_per_target_and_case_insensitive() {
        assert_eq!(
            chap_scope(
                "iqn.2026-11.local.homevault:disk0",
                "IQN.2026-11.local.client:a"
            ),
            chap_scope(
                "IQN.2026-11.local.homevault:disk0",
                "iqn.2026-11.local.client:A"
            )
        );
        assert_ne!(
            chap_scope(
                "iqn.2026-11.local.homevault:disk0",
                "iqn.2026-11.local.client:a"
            ),
            chap_scope(
                "iqn.2026-11.local.homevault:disk1",
                "iqn.2026-11.local.client:a"
            )
        );
    }

    fn backstore(id: i64, name: &str) -> IscsiBackstore {
        IscsiBackstore {
            id,
            name: name.to_string(),
            kind: "fileio".to_string(),
            path: format!("/var/lib/homevault/iscsi/{name}.img"),
            size_bytes: 1024 * 1024,
            readonly: false,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
            lun_count: 0,
        }
    }

    fn target() -> IscsiTarget {
        IscsiTarget {
            id: 1,
            iqn: "iqn.2026-11.local.homevault:disk0".to_string(),
            enabled: true,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
            portals: vec![IscsiPortal {
                id: 1,
                target_id: 1,
                address: "0.0.0.0".to_string(),
                port: 3260,
            }],
            luns: vec![IscsiLun {
                id: 1,
                target_id: 1,
                lun: 0,
                backstore_id: 1,
                backstore_name: "data".to_string(),
            }],
            acls: vec![IscsiAcl {
                id: 1,
                target_id: 1,
                initiator_iqn: "iqn.2026-11.local.client:laptop".to_string(),
                userid: Some("homevault".to_string()),
                has_secret: true,
            }],
        }
    }

    #[test]
    fn test_backstore_names_are_safe_object_names() {
        assert!(valid_backstore_name("data"));
        assert!(valid_backstore_name("vm_disk-1.2"));
        assert!(!valid_backstore_name(""));
        assert!(!valid_backstore_name(".hidden"));
        assert!(!valid_backstore_name("-leading"));
        assert!(!valid_backstore_name("has space"));
        assert!(!valid_backstore_name("has/slash"));
        assert!(!valid_backstore_name("has;semicolon"));
        assert!(!valid_backstore_name("default"));
        assert!(!valid_backstore_name(&"a".repeat(MAX_BACKSTORE_NAME + 1)));
    }

    #[test]
    fn test_iqn_validation() {
        assert!(valid_iqn("iqn.2026-11.local.homevault:disk0"));
        assert!(valid_iqn("eui.02004567A425678D"));
        assert!(valid_iqn("naa.6001405c3213d4e5"));
        // No scheme at all, a bare IQN prefix, a space, and traversal.
        assert!(!valid_iqn("disk0"));
        assert!(!valid_iqn("iqn.2026-11"));
        assert!(!valid_iqn("iqn.2026-11.local home"));
        assert!(!valid_iqn("iqn.2026-11.local..home"));
        assert!(!valid_iqn(&format!(
            "iqn.2026-11.{}",
            "a".repeat(MAX_IQN_LEN)
        )));
    }

    #[test]
    fn test_portal_and_lun_bounds() {
        assert!(valid_portal_address("0.0.0.0"));
        assert!(valid_portal_address("192.168.1.10"));
        assert!(valid_portal_address("::"));
        assert!(!valid_portal_address("nas.local"));
        assert!(!valid_portal_address("192.168.1.999"));
        assert!(valid_portal_port(3260));
        assert!(!valid_portal_port(0));
        assert!(!valid_portal_port(65536));
        assert!(valid_lun(0));
        assert!(valid_lun(MAX_LUN));
        assert!(!valid_lun(-1));
        assert!(!valid_lun(MAX_LUN + 1));
    }

    #[test]
    fn test_chap_needs_both_halves_and_a_spec_sized_secret() {
        assert!(validate_chap(None, None).is_ok());
        assert!(validate_chap(Some("homevault"), Some("TwelveChars!")).is_ok());
        assert!(validate_chap(Some("homevault"), None).is_err());
        assert!(validate_chap(None, Some("TwelveChars!")).is_err());
        // 11 chars is under the RFC floor, 17 over the ceiling.
        assert!(validate_chap(Some("u"), Some("ElevenChars")).is_err());
        assert!(validate_chap(Some("u"), Some("SeventeenChars!!!")).is_err());
        assert!(validate_chap(Some(""), Some("TwelveChars!")).is_err());
        assert!(validate_chap(Some("u"), Some("Twelve Chars")).is_err());
    }

    #[test]
    fn test_validate_target_covers_every_collection() {
        let iqn = "iqn.2026-11.local.homevault:disk0";
        let portals = vec![("0.0.0.0".to_string(), 3260)];
        assert!(validate_target(iqn, &portals, &[0], &[]).is_ok());

        assert!(validate_target("nope", &portals, &[0], &[]).is_err());
        assert!(
            validate_target(iqn, &[], &[0], &[]).is_err(),
            "a target with no portal is unreachable"
        );
        assert!(validate_target(
            iqn,
            &[("0.0.0.0".to_string(), 3260), ("0.0.0.0".to_string(), 3260)],
            &[0],
            &[]
        )
        .is_err());
        assert!(validate_target(iqn, &portals, &[0, 0], &[]).is_err());
        assert!(validate_target(iqn, &portals, &[MAX_LUN + 1], &[]).is_err());
        assert!(validate_target(
            iqn,
            &portals,
            &[0],
            &[
                ("iqn.2026-11.local.client:a".to_string(), None, None),
                ("iqn.2026-11.local.client:A".to_string(), None, None)
            ]
        )
        .is_err());
        assert!(validate_target(
            iqn,
            &portals,
            &[0],
            &[(
                "iqn.2026-11.local.client:a".to_string(),
                Some("u".to_string()),
                Some("short".to_string())
            )]
        )
        .is_err());
    }

    #[test]
    fn test_backstore_path_must_be_absolute_and_traversal_free() {
        assert!(validate_backstore_path("/var/lib/homevault/iscsi/a.img").is_ok());
        assert!(validate_backstore_path("").is_err());
        assert!(validate_backstore_path("relative/a.img").is_err());
        assert!(validate_backstore_path("/var/lib/../../etc/passwd").is_err());
        assert!(validate_backstore_path("/var/lib/a\0b").is_err());
    }

    #[test]
    fn test_fileio_and_block_backstores_get_distinct_commands() {
        let mut fileio = backstore(1, "data");
        fileio.size_bytes = 5 * 1024 * 1024;
        let line = backstore_create_command(&fileio).describe();
        assert!(line.contains("/backstores/fileio create data"));
        assert!(line.contains("5242880"));
        assert!(line.contains("readonly=0"));

        let mut block = backstore(2, "raw");
        block.kind = "block".to_string();
        block.path = "/dev/sdb1".to_string();
        block.readonly = true;
        let line = backstore_create_command(&block).describe();
        assert!(line.contains("/backstores/block create raw dev=/dev/sdb1"));
        assert!(line.contains("readonly=1"));
    }

    #[test]
    fn test_backstore_delete_names_the_object_and_not_the_file() {
        let store = backstore(1, "data");
        let line = backstore_delete_command(&store).describe();
        assert_eq!(
            line, "sudo targetcli /backstores/fileio delete data",
            "forgetting a backstore must not touch the backing file"
        );
    }

    #[test]
    fn test_target_commands_order_portals_luns_acls_then_enable_last() {
        let stores = vec![backstore(1, "data")];
        let commands: Vec<String> = target_apply_commands(
            &target(),
            &stores,
            &[(
                "iqn.2026-11.local.homevault:disk0".to_string(),
                "iqn.2026-11.local.client:laptop".to_string(),
                "TwelveChars!".to_string(),
            )],
        )
        .iter()
        .map(IscsiCommand::describe)
        .collect();
        let joined = commands.join("\n");
        let pos = |needle: &str| {
            joined
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} missing"))
        };

        assert!(pos("/iscsi create iqn.2026-11.local.homevault:disk0") < pos("portals create"));
        assert!(pos("portals create") < pos("/luns create /backstores/fileio/data 0"));
        assert!(pos("/luns create") < pos("acls create"));
        assert!(pos("acls create") < pos("demo_mode_write_protect=0"));
        assert!(
            commands
                .last()
                .is_some_and(|line| line.ends_with("/tpg1 enable")),
            "enable must be last, so an initiator cannot log in to a half-built target"
        );
        assert!(
            pos("demo_mode_write_protect=0") < pos("/tpg1 enable"),
            "the write-protect override must be set before the target is enabled"
        );
        // CHAP userid travels, the secret is redacted everywhere and never
        // rendered into the report.
        assert!(joined.contains("userid=homevault"));
        assert!(joined.contains("password=***"));
        assert!(!joined.contains("TwelveChars!"));
    }

    #[test]
    fn test_a_lun_whose_backstore_vanished_is_skipped() {
        let mut t = target();
        t.luns[0].backstore_id = 42;
        let commands = target_apply_commands(&t, &[], &[]);
        let joined: Vec<String> = commands.iter().map(IscsiCommand::describe).collect();
        assert!(
            !joined.iter().any(|c| c.contains("/luns create")),
            "a LUN must never point at a backstore object that does not exist"
        );
    }

    #[test]
    fn test_teardown_names_exactly_one_iqn() {
        assert_eq!(
            target_teardown_command("iqn.2026-11.local.homevault:disk0").describe(),
            "sudo targetcli /iscsi delete iqn.2026-11.local.homevault:disk0"
        );
    }

    #[test]
    fn test_module_commands_only_cover_the_missing_ones() {
        let commands = module_commands(&["target_core_mod".to_string()]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].describe(), "sudo modprobe target_core_mod");
        assert!(module_commands(&[]).is_empty());
    }

    #[test]
    fn test_redaction_leaves_other_key_values_alone() {
        assert_eq!(
            redact("sudo targetcli /iscsi/x/tpg1/acls/y set auth password=Secret123456"),
            "sudo targetcli /iscsi/x/tpg1/acls/y set auth password=***"
        );
        assert_eq!(
            redact("sudo targetcli /iscsi/x/tpg1/acls/y set auth userid=homevault"),
            "sudo targetcli /iscsi/x/tpg1/acls/y set auth userid=homevault"
        );
    }

    #[test]
    fn test_deleting_a_backing_file_refuses_directories_and_relative_paths() {
        assert!(delete_backing_file("relative.img").is_err());
        assert!(delete_backing_file("/").is_err());
        // A path that does not exist is already gone: metadata deletion wins.
        assert!(delete_backing_file("/nonexistent/hv-iscsi-test.img").is_ok());
    }

    #[test]
    fn test_settings_default_to_off() {
        let settings = IscsiSettings::default();
        assert!(!settings.enabled);
        assert!(settings.updated_at.is_empty());
        assert!(!new_settings(false).enabled);
        assert!(new_settings(true).enabled);
    }
}
