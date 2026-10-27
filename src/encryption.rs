/* =============================================================================
 * File          : encryption.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   LUKS2 disk encryption for standalone, unassigned block devices: create
 *   (format + passphrase), open/close (mapper management), passphrase
 *   rotation and destroy. Deliberately scoped to single devices — not RAID
 *   arrays or ZFS pools, which have their own native encryption paths.
 *
 *   Every privileged call is bounded by a deadline and killed on timeout
 *   (ROADMAP 9), and key-slot erasure is gated on the volume really being
 *   released first (ROADMAP 9.2).
 *
 * Dependencies:
 *   - internal disk service (device listing)
 *
 * Usage:
 *   handlers/encryption.rs; metadata persisted via EncryptedVolumeRepository.
 * =============================================================================
 */

//! LUKS2 disk encryption for standalone, available (unassigned) block devices.
//!
//! Deliberately scoped to a single device at a time — not RAID arrays or
//! ZFS pools (ZFS already has native dataset encryption, a separate,
//! more idiomatic mechanism). Unlock is always manual: no key material is
//! ever stored on the server, so every volume stays locked after a reboot
//! until an admin re-enters the passphrase through the UI. Passphrases are
//! always passed to `cryptsetup` via stdin, never as a CLI argument, so
//! they never appear in the process list.
//!
//! Three properties this module owns (ROADMAP item 9):
//!
//! 1. **No unbounded subprocess.** `luksFormat` alone can run for minutes on a
//!    multi-terabyte device, and a wedged `mount`/`cryptsetup` would otherwise
//!    hold a handler (or a job) forever. Every call goes through [`run_timed`],
//!    which sets `kill_on_drop` so a deadline really kills the child.
//! 2. **Erasure is gated.** `cryptsetup erase` destroys the key slots: the data
//!    is unrecoverable afterwards even though the filesystem may still be
//!    mounted and being written to. [`erase_gate`] refuses to proceed unless
//!    the kernel holds neither the mount nor the mapper, and the caller must
//!    pass the *verified* state (from `/proc/self/mounts` and `cryptsetup
//!    status`), never the exit code of the command it hoped would release it.
//! 3. **Rotation exists and is verified.** [`change_passphrase`] feeds
//!    `luksChangeKey` the three lines it actually prompts for (old, new,
//!    confirmation) and then proves the new passphrase opens the container
//!    with `open --test-passphrase`.

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("`{command}` did not finish within {secs}s")]
    Timeout { command: String, secs: u64 },
    #[error("Volume is still in use: {0}")]
    StillInUse(String),
}

// ---------------------------------------------------------------------------
// Deadlines (ROADMAP 9 / X.2)
//
// Each value is generous for the slowest honest case on a home appliance and
// short enough that a hung tool becomes a visible, retryable failure instead
// of a wedged job. `luksFormat` is the only genuinely slow one (it writes a
// fresh header and, with `--batch-mode`, no entropy gathering of its own).
// ---------------------------------------------------------------------------

/// `cryptsetup luksFormat` — minutes on a multi-terabyte device.
pub const LUKS_FORMAT_TIMEOUT: Duration = Duration::from_secs(900);
/// `cryptsetup open` — PBKDF2/argon2 derivation plus the mapper setup.
pub const LUKS_OPEN_TIMEOUT: Duration = Duration::from_secs(180);
/// `cryptsetup close` — fails fast while the device is busy.
pub const LUKS_CLOSE_TIMEOUT: Duration = Duration::from_secs(120);
/// `cryptsetup erase` — wipes the key slots of the header.
pub const LUKS_ERASE_TIMEOUT: Duration = Duration::from_secs(600);
/// `cryptsetup status` — a header read, used on every list.
pub const LUKS_STATUS_TIMEOUT: Duration = Duration::from_secs(15);
/// `cryptsetup luksUUID` — a header read.
pub const LUKS_UUID_TIMEOUT: Duration = Duration::from_secs(30);
/// `cryptsetup luksChangeKey` — re-derives the key for one slot.
pub const LUKS_CHANGE_KEY_TIMEOUT: Duration = Duration::from_secs(180);
/// `mount`/`umount` — a busy unmount returns immediately.
pub const MOUNT_TIMEOUT: Duration = Duration::from_secs(120);
/// `sudo tee /etc/fstab` — a tiny local write.
pub const FSTAB_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the kernel reports the live mount table.
const MOUNTS_PATH: &str = "/proc/self/mounts";

/// The one file this module rewrites as root.
pub const FSTAB_PATH: &str = "/etc/fstab";

// ---------------------------------------------------------------------------
// Passphrase policy
// ---------------------------------------------------------------------------

/// Minimum passphrase length enforced on **creation** and on **rotation to a
/// new passphrase**. It is deliberately not applied when unlocking an existing
/// volume: a volume created under the old 8-character rule must stay usable.
pub const MIN_PASSPHRASE_LEN: usize = 12;

/// Why a candidate passphrase was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassphraseProblem {
    /// Shorter than [`MIN_PASSPHRASE_LEN`] characters.
    TooShort,
    /// On the small list of passphrases a wordlist attack tries first, or the
    /// volume label itself.
    TooCommon,
    /// Fewer than four distinct characters ("aaaaaaaaaaaa").
    TooSimple,
}

impl PassphraseProblem {
    /// Operator-facing sentence. English, like every other API error; the UI
    /// translates its own pre-flight messages from the same rule.
    pub fn message(&self) -> String {
        match self {
            PassphraseProblem::TooShort => format!(
                "Passphrase must be at least {MIN_PASSPHRASE_LEN} characters"
            ),
            PassphraseProblem::TooCommon => {
                "That passphrase is too common — it is one of the first guesses an attacker tries. Choose something else.".to_string()
            }
            PassphraseProblem::TooSimple => {
                "That passphrase has too little variety — use at least four different characters.".to_string()
            }
        }
    }
}

/// The handful of passphrases a wordlist starts with. Not a strength meter:
/// this only catches the "I typed the first thing that came to mind" case that
/// a 12-character minimum would otherwise wave through.
const COMMON_PASSPHRASES: &[&str] = &[
    "password1234",
    "password12345",
    "123456789012",
    "1234567890123",
    "qwertyuiop12",
    "letmein12345",
    "iloveyou1234",
    "homevault123",
    "changeme1234",
    "administrator",
];

/// Checks a **new** passphrase against the policy above.
///
/// `label` is the volume label: a passphrase equal to it is refused, because
/// the label is stored in clear text in the metadata row and shown on the
/// page — it is not a secret.
pub fn validate_passphrase(passphrase: &str, label: &str) -> Result<(), PassphraseProblem> {
    if passphrase.chars().count() < MIN_PASSPHRASE_LEN {
        return Err(PassphraseProblem::TooShort);
    }
    let lowered = passphrase.trim().to_lowercase();
    if COMMON_PASSPHRASES.contains(&lowered.as_str())
        || (!label.trim().is_empty() && lowered == label.trim().to_lowercase())
    {
        return Err(PassphraseProblem::TooCommon);
    }
    let mut distinct = Vec::new();
    for c in passphrase.chars() {
        if !distinct.contains(&c) {
            distinct.push(c);
            if distinct.len() >= 4 {
                return Ok(());
            }
        }
    }
    Err(PassphraseProblem::TooSimple)
}

/// The exact stdin `cryptsetup luksChangeKey` expects: the current passphrase,
/// then the new one **twice** (the confirmation prompt). The previous version
/// of this function sent it once, so every call would have died on
/// "Passphrases do not match" — one more reason rotation had no caller.
pub fn change_key_stdin(old_passphrase: &str, new_passphrase: &str) -> String {
    format!("{old_passphrase}\n{new_passphrase}\n{new_passphrase}\n")
}

// ---------------------------------------------------------------------------
// Privileged command execution
// ---------------------------------------------------------------------------

/// `program arg1 arg2 …` for error messages and logs.
fn display_command(program: &str, args: &[&str]) -> String {
    let mut rendered = program.to_string();
    for arg in args {
        rendered.push(' ');
        rendered.push_str(arg);
    }
    rendered
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// Turns a non-zero exit into [`EncryptionError::CommandFailed`] carrying the
/// tool's own stderr (which is what the operator needs to see).
fn ensure_success(output: Output) -> Result<Output, EncryptionError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(EncryptionError::CommandFailed(stderr_of(&output)))
    }
}

/// Runs `program args…`, optionally feeding `stdin_data`, under a deadline.
///
/// `kill_on_drop(true)` is the part that matters: a plain `Command::output()`
/// future dropped on timeout leaves the child running, so "timed out" would
/// only stop us from waiting while a wedged `cryptsetup` kept holding the
/// device (and the next attempt collided with it).
async fn run_timed(
    program: &str,
    args: &[&str],
    stdin_data: Option<&str>,
    timeout: Duration,
) -> Result<Output, EncryptionError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let payload = stdin_data.map(|s| s.to_string());
    let work = async move {
        if let Some(payload) = payload {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(payload.as_bytes()).await?;
                stdin.flush().await?;
            }
        }
        child.wait_with_output().await
    };

    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(EncryptionError::Io(e)),
        Err(_) => Err(EncryptionError::Timeout {
            command: display_command(program, args),
            secs: timeout.as_secs(),
        }),
    }
}

/// [`run_timed`] behind `sudo` — every privileged call in this module is
/// sudo-wrapped, so the deadline belongs on the same argv.
async fn sudo_timed(
    args: &[&str],
    stdin_data: Option<&str>,
    timeout: Duration,
) -> Result<Output, EncryptionError> {
    run_timed("sudo", args, stdin_data, timeout).await
}

/// Runs a sudo-wrapped `cryptsetup` subcommand, failing on a non-zero exit.
async fn cryptsetup(
    args: &[&str],
    stdin_data: Option<&str>,
    timeout: Duration,
) -> Result<Output, EncryptionError> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("cryptsetup");
    argv.extend_from_slice(args);
    ensure_success(sudo_timed(&argv, stdin_data, timeout).await?)
}

// ---------------------------------------------------------------------------
// Volume operations
// ---------------------------------------------------------------------------

/// Formats `device` as a new LUKS2 container with `passphrase`.
///
/// Kept separate from [`open`] so the creation job knows whether a container
/// exists yet: a failure before the header is written must not trigger an
/// erasure, and a failure after it must (the device is no longer "available"
/// and has no metadata row to retry from).
pub async fn format_luks(device: &str, passphrase: &str) -> Result<(), EncryptionError> {
    cryptsetup(
        &["luksFormat", "--type", "luks2", "--batch-mode", device, "-"],
        Some(passphrase),
        LUKS_FORMAT_TIMEOUT,
    )
    .await?;
    Ok(())
}

/// Unlocks an already-formatted LUKS device under `mapper_name`.
pub async fn open(
    device: &str,
    mapper_name: &str,
    passphrase: &str,
) -> Result<(), EncryptionError> {
    cryptsetup(
        &["open", device, mapper_name, "--key-file", "-"],
        Some(passphrase),
        LUKS_OPEN_TIMEOUT,
    )
    .await?;
    Ok(())
}

/// Proves `passphrase` opens `device`, without mapping it
/// (`cryptsetup open --test-passphrase`). Used after a rotation so a silent
/// `luksChangeKey` failure can never leave the owner locked out.
pub async fn verify_passphrase(device: &str, passphrase: &str) -> Result<(), EncryptionError> {
    cryptsetup(
        &["open", "--test-passphrase", "--key-file", "-", device],
        Some(passphrase),
        LUKS_OPEN_TIMEOUT,
    )
    .await?;
    Ok(())
}

/// Locks (closes) a mapper device. Idempotent — succeeds even if already closed.
pub async fn close(mapper_name: &str) -> Result<(), EncryptionError> {
    let output = sudo_timed(
        &["cryptsetup", "close", mapper_name],
        None,
        LUKS_CLOSE_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        let stderr = stderr_of(&output);
        // Already closed / not active is not a real error for a "lock" action.
        if stderr.contains("not active") {
            return Ok(());
        }
        return Err(EncryptionError::CommandFailed(stderr));
    }
    Ok(())
}

/// Returns true if `mapper_name` is currently open/unlocked.
///
/// A failure to *ask* (tool missing, sudo refused, timeout) is reported as
/// "not open": every caller either presents it as a UI badge or has already
/// released the volume, and [`erase_gate`] re-verifies the state it cares
/// about through [`is_mounted`].
pub async fn is_open(mapper_name: &str) -> bool {
    match sudo_timed(
        &["cryptsetup", "status", mapper_name],
        None,
        LUKS_STATUS_TIMEOUT,
    )
    .await
    {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Returns the LUKS UUID of a formatted device.
pub async fn luks_uuid(device: &str) -> Result<String, EncryptionError> {
    let output = cryptsetup(&["luksUUID", device], None, LUKS_UUID_TIMEOUT).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Mounts the (already-unlocked) mapper device at `mount_point`.
pub async fn mount(mapper_name: &str, mount_point: &str) -> Result<(), EncryptionError> {
    // `sudo mkdir`, not `std::fs::create_dir_all` — this runs under
    // ENCRYPTED_MOUNT_BASE, root-owned, and sudo only wraps external Command
    // invocations, not Rust's own syscalls (same fix as pools.rs).
    let mkdir = sudo_timed(&["mkdir", "-p", mount_point], None, MOUNT_TIMEOUT).await?;
    if !mkdir.status.success() {
        return Err(EncryptionError::CommandFailed(format!(
            "Failed to create mount point: {}",
            stderr_of(&mkdir)
        )));
    }
    let device = format!("/dev/mapper/{}", mapper_name);
    let output = sudo_timed(&["mount", &device, mount_point], None, MOUNT_TIMEOUT).await?;
    ensure_success(output)?;
    Ok(())
}

/// Unmounts a mount point. Idempotent — succeeds even if not mounted.
pub async fn unmount(mount_point: &str) -> Result<(), EncryptionError> {
    let output = sudo_timed(&["umount", mount_point], None, MOUNT_TIMEOUT).await?;
    if !output.status.success() {
        let stderr = stderr_of(&output);
        if stderr.contains("not mounted") {
            return Ok(());
        }
        return Err(EncryptionError::CommandFailed(stderr));
    }
    Ok(())
}

/// Changes an existing volume's passphrase, then proves the new one works.
pub async fn change_passphrase(
    device: &str,
    old_passphrase: &str,
    new_passphrase: &str,
) -> Result<(), EncryptionError> {
    cryptsetup(
        &["luksChangeKey", device],
        Some(&change_key_stdin(old_passphrase, new_passphrase)),
        LUKS_CHANGE_KEY_TIMEOUT,
    )
    .await?;
    verify_passphrase(device, new_passphrase).await
}

/// What [`erase`] managed to do beyond destroying the key slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseOutcome {
    /// Whether the LUKS signature was cleared, so the device returns to the
    /// available pool. `false` means the key slots are gone (the data is
    /// unrecoverable) but the device still looks like a LUKS container.
    pub signature_cleared: bool,
    /// A sentence to surface to the admin when `signature_cleared` is false.
    pub warning: Option<String>,
}

/// Permanently destroys a LUKS volume's key slots, making its data
/// unrecoverable. Callers **must** prove the volume is released first — see
/// [`erase_gate`]; this function does not unmount or close anything.
pub async fn erase(device: &str) -> Result<EraseOutcome, EncryptionError> {
    cryptsetup(&["erase", "--batch-mode", device], None, LUKS_ERASE_TIMEOUT).await?;

    // `cryptsetup erase` wipes the key slots (data is now unrecoverable) but
    // leaves the LUKS2 header structure — so `blkid`/lsblk still report
    // `crypto_LUKS` and `DiskService::get_available_devices` keeps treating
    // the device as in-use, meaning the admin can never re-encrypt or
    // repurpose it through HomeVault. Clear the signature so it returns to
    // the available pool. The security-critical step already succeeded, so a
    // failure here is reported as a warning instead of being swallowed: the
    // device needs a manual `wipefs` before it can be reused.
    match sudo_timed(&["wipefs", "-a", device], None, LUKS_ERASE_TIMEOUT).await {
        Ok(output) if output.status.success() => Ok(EraseOutcome {
            signature_cleared: true,
            warning: None,
        }),
        Ok(output) => Ok(EraseOutcome {
            signature_cleared: false,
            warning: Some(format!(
                "The LUKS key slots are destroyed, but the signature could not be cleared ({}). The device still shows as in use — wipe it by hand (`sudo wipefs -a {device}`) before reusing it.",
                stderr_of(&output)
            )),
        }),
        Err(e) => Ok(EraseOutcome {
            signature_cleared: false,
            warning: Some(format!(
                "The LUKS key slots are destroyed, but the signature could not be cleared ({e}). The device still shows as in use — wipe it by hand (`sudo wipefs -a {device}`) before reusing it."
            )),
        }),
    }
}

// ---------------------------------------------------------------------------
// Release gate (ROADMAP 9.2)
// ---------------------------------------------------------------------------

/// Why a volume may not have its key slots erased yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseBlocker {
    /// The mount point is still present in the kernel's mount table.
    Mounted(String),
    /// The mapper device is still open.
    Open(String),
}

impl std::fmt::Display for ReleaseBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReleaseBlocker::Mounted(mount_point) => write!(
                f,
                "the volume is still mounted at {mount_point} — close whatever is using it and try again"
            ),
            ReleaseBlocker::Open(mapper_name) => write!(
                f,
                "the mapper device {mapper_name} is still open — it could not be closed, so nothing was erased"
            ),
        }
    }
}

/// The last gate before `cryptsetup erase`.
///
/// Erasing the key slots while the filesystem is still mounted and being
/// written to destroys the only way back to the data — the same class of
/// unrecoverable loss the RAID teardown was fixed for (ROADMAP 8.1). The
/// caller passes the **verified** state (`Some(mount point)` when the kernel
/// still lists it, `Some(mapper name)` when the device is still open), never
/// "the umount command exited 0".
pub fn erase_gate(mounted: Option<&str>, open: Option<&str>) -> Result<(), ReleaseBlocker> {
    if let Some(mount_point) = mounted {
        return Err(ReleaseBlocker::Mounted(mount_point.to_string()));
    }
    if let Some(mapper_name) = open {
        return Err(ReleaseBlocker::Open(mapper_name.to_string()));
    }
    Ok(())
}

/// Decodes the octal escapes the kernel writes into `/proc/self/mounts`
/// (`\040` space, `\011` tab, `\012` newline, `\134` backslash).
pub fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            if digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
                let code = (digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0');
                out.push(code);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// True when `mount_point` is present in a `/proc/self/mounts` body. Pure so
/// the parsing is pinned by tests; the mount point is compared as a whole
/// field (a prefix like `/mnt/encrypted/vault2` must not match
/// `/mnt/encrypted/vault`).
pub fn is_mounted_in(mounts: &str, mount_point: &str) -> bool {
    let wanted = mount_point.trim_end_matches('/');
    mounts.lines().any(|line| {
        line.split_whitespace()
            .nth(1)
            .map(|field| unescape_mount_field(field).trim_end_matches('/') == wanted)
            .unwrap_or(false)
    })
}

/// True when `mount_point` is mounted right now.
///
/// Reads the kernel's own table rather than trusting the last `umount` exit
/// status. An unreadable `/proc/self/mounts` answers **true** (fail closed):
/// refusing an erase is recoverable, erasing a live volume is not.
pub async fn is_mounted(mount_point: &str) -> bool {
    match tokio::fs::read_to_string(MOUNTS_PATH).await {
        Ok(mounts) => is_mounted_in(&mounts, mount_point),
        Err(e) => {
            tracing::warn!(
                "encryption: cannot read {MOUNTS_PATH} to verify that {mount_point} is unmounted: {e}"
            );
            true
        }
    }
}

// ---------------------------------------------------------------------------
// /etc/fstab persistence
// ---------------------------------------------------------------------------

/// Marker comment prefix used to make fstab edits idempotent and reversible,
/// mirroring the technique used elsewhere in the codebase for RAID mounts —
/// written locally here (not reused) since that helper lives in a frozen file.
fn fstab_marker(label: &str) -> String {
    format!("# homevault-encrypted:{}", label)
}

/// Pure edit: returns the new `/etc/fstab` body with the marked entry added,
/// or `None` when the marker is already present (idempotent).
///
/// The entry is `noauto`: the volume must never be auto-mounted at boot since
/// it is still locked at that point.
pub fn with_marked_entry(
    existing: &str,
    mapper_name: &str,
    mount_point: &str,
    filesystem: &str,
    label: &str,
) -> Option<String> {
    let marker = fstab_marker(label);
    if existing.contains(&marker) {
        return None;
    }
    let line = format!(
        "{}\n/dev/mapper/{} {} {} noauto,defaults 0 0\n",
        marker, mapper_name, mount_point, filesystem
    );
    let mut updated = existing.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&line);
    Some(updated)
}

/// Pure edit: drops the marker line and the line immediately after it, if
/// present. Every other line is preserved verbatim.
pub fn without_marked_entry(existing: &str, label: &str) -> String {
    let marker = fstab_marker(label);
    let mut out: Vec<&str> = Vec::new();
    let mut skip_next = false;
    for line in existing.lines() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if line.trim() == marker.trim() {
            skip_next = true;
            continue;
        }
        out.push(line);
    }
    let mut content = out.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    content
}

/// Writes `content` to `path` via `sudo tee`, not a direct `std::fs` write —
/// `/etc/fstab` is `644 root:root`, so a direct write from the unprivileged
/// homevault process fails with EACCES. Bounded by [`FSTAB_TIMEOUT`].
async fn write_file_as_root(path: &Path, content: &str) -> Result<(), EncryptionError> {
    let path = path.to_string_lossy().to_string();
    let output = sudo_timed(&["tee", &path], Some(content), FSTAB_TIMEOUT).await?;
    ensure_success(output)?;
    Ok(())
}

/// Reads `path`, tolerating a missing file (a fresh appliance may not have an
/// `/etc/fstab` yet).
async fn read_fstab(path: &Path) -> Result<String, EncryptionError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(EncryptionError::Io(e)),
    }
}

/// Adds the marked `noauto` fstab entry for `label`, if not already present.
pub async fn append_fstab_entry(
    mapper_name: &str,
    mount_point: &str,
    filesystem: &str,
    label: &str,
) -> Result<(), EncryptionError> {
    let path = Path::new(FSTAB_PATH);
    let existing = read_fstab(path).await?;
    match with_marked_entry(&existing, mapper_name, mount_point, filesystem, label) {
        None => Ok(()),
        Some(updated) => write_file_as_root(path, &updated).await,
    }
}

/// Removes the marked fstab entry for `label`, if present.
pub async fn remove_fstab_entry(label: &str) -> Result<(), EncryptionError> {
    let path = Path::new(FSTAB_PATH);
    if !path.exists() {
        return Ok(());
    }
    let existing = read_fstab(path).await?;
    let updated = without_marked_entry(&existing, label);
    if updated == existing {
        return Ok(());
    }
    write_file_as_root(path, &updated).await
}

#[cfg(test)]
mod tests {
    //! `encryption.rs` is a thin wrapper around `cryptsetup`/`mount` (all side
    //! effects, all root). What *is* testable here is exactly what ROADMAP
    //! item 9 was about: the passphrase policy, the erasure gate, the mount
    //! table parsing and the fstab edits — plus the two properties of
    //! `run_timed` (it returns a fast command's output, it kills a hung one)
    //! that keep a wedged `cryptsetup` from parking a job.
    use super::*;

    // -- fstab marker + edits ------------------------------------------------

    #[test]
    fn test_fstab_marker_format_is_stable() {
        assert_eq!(fstab_marker("photos"), "# homevault-encrypted:photos");
        assert_eq!(fstab_marker("vault-01"), "# homevault-encrypted:vault-01");
    }

    #[test]
    fn test_fstab_marker_is_distinct_per_label() {
        assert_ne!(fstab_marker("a"), fstab_marker("b"));
    }

    #[test]
    fn test_with_marked_entry_appends_marker_and_noauto_line() {
        let updated = with_marked_entry(
            "/dev/sda1 / ext4 defaults 0 1\n",
            "hv-1",
            "/mnt/encrypted/vault",
            "ext4",
            "vault",
        )
        .expect("a fresh entry is appended");
        assert_eq!(
            updated,
            "/dev/sda1 / ext4 defaults 0 1\n\
             # homevault-encrypted:vault\n\
             /dev/mapper/hv-1 /mnt/encrypted/vault ext4 noauto,defaults 0 0\n"
        );
    }

    #[test]
    fn test_with_marked_entry_is_idempotent() {
        let once = with_marked_entry("", "hv-1", "/mnt/encrypted/v", "ext4", "v").unwrap();
        assert_eq!(
            with_marked_entry(&once, "hv-1", "/mnt/encrypted/v", "ext4", "v"),
            None
        );
    }

    #[test]
    fn test_with_marked_entry_keeps_a_missing_trailing_newline() {
        let updated = with_marked_entry(
            "/dev/sda1 / ext4 defaults 0 1",
            "hv-1",
            "/mnt/x",
            "xfs",
            "x",
        )
        .expect("appended");
        assert!(
            updated.starts_with("/dev/sda1 / ext4 defaults 0 1\n# homevault-encrypted:x\n"),
            "{updated}"
        );
    }

    #[test]
    fn test_without_marked_entry_removes_only_its_own_pair() {
        let existing = "\
/dev/sda1 / ext4 defaults 0 1
# homevault-encrypted:keep
/dev/mapper/hv-keep /mnt/encrypted/keep ext4 noauto,defaults 0 0
# homevault-encrypted:drop
/dev/mapper/hv-drop /mnt/encrypted/drop ext4 noauto,defaults 0 0
";
        let updated = without_marked_entry(existing, "drop");
        assert_eq!(
            updated,
            "/dev/sda1 / ext4 defaults 0 1\n\
             # homevault-encrypted:keep\n\
             /dev/mapper/hv-keep /mnt/encrypted/keep ext4 noauto,defaults 0 0\n"
        );
    }

    #[test]
    fn test_without_marked_entry_leaves_an_unrelated_file_untouched() {
        let existing = "/dev/sda1 / ext4 defaults 0 1\n";
        assert_eq!(without_marked_entry(existing, "vault"), existing);
    }

    // -- passphrase policy ---------------------------------------------------

    #[test]
    fn test_validate_passphrase_accepts_a_long_mixed_passphrase() {
        assert_eq!(
            validate_passphrase("correct horse battery", "vault"),
            Ok(())
        );
        assert_eq!(validate_passphrase("Tr0ub4dor&3-x", "vault"), Ok(()));
    }

    #[test]
    fn test_validate_passphrase_rejects_anything_under_the_minimum() {
        for short in ["", "short", "abcdefghij", "abcdefghijk"] {
            assert_eq!(
                validate_passphrase(short, "vault"),
                Err(PassphraseProblem::TooShort),
                "{short:?} must be refused"
            );
        }
        assert_eq!(
            validate_passphrase(&"a".repeat(MIN_PASSPHRASE_LEN - 1), "v"),
            Err(PassphraseProblem::TooShort)
        );
    }

    #[test]
    fn test_validate_passphrase_rejects_the_wordlist_and_the_label() {
        for common in [
            "password1234",
            "PASSWORD12345",
            "123456789012",
            " changeme1234 ",
        ] {
            assert_eq!(
                validate_passphrase(common, "vault"),
                Err(PassphraseProblem::TooCommon),
                "{common:?} must be refused"
            );
        }
        assert_eq!(
            validate_passphrase("my-vault-label", "my-vault-label"),
            Err(PassphraseProblem::TooCommon),
            "the label is stored in clear text, it cannot be the passphrase"
        );
    }

    #[test]
    fn test_validate_passphrase_rejects_a_low_variety_one() {
        assert_eq!(
            validate_passphrase("aaaaaaaaaaaaaaaa", "vault"),
            Err(PassphraseProblem::TooSimple)
        );
        assert_eq!(
            validate_passphrase("abababababababab", "vault"),
            Err(PassphraseProblem::TooSimple)
        );
        // Repetition is fine as long as the variety rule is met: four
        // distinct characters (`a`, `b`, `c`, `d`) clears it, three does not.
        assert_eq!(validate_passphrase("abcdabcdabcd", "vault"), Ok(()));
    }

    #[test]
    fn test_change_key_stdin_carries_the_confirmation_line() {
        assert_eq!(
            change_key_stdin("old-passphrase", "new-passphrase"),
            "old-passphrase\nnew-passphrase\nnew-passphrase\n"
        );
    }

    // -- release gate --------------------------------------------------------

    #[test]
    fn test_erase_gate_refuses_while_the_volume_is_mounted() {
        assert_eq!(
            erase_gate(Some("/mnt/encrypted/vault"), None),
            Err(ReleaseBlocker::Mounted("/mnt/encrypted/vault".into()))
        );
        assert_eq!(
            erase_gate(Some("/mnt/encrypted/vault"), Some("hv-1")),
            Err(ReleaseBlocker::Mounted("/mnt/encrypted/vault".into())),
            "a live mount is the worse fact and must be reported first"
        );
    }

    #[test]
    fn test_erase_gate_refuses_while_the_mapper_is_open() {
        assert_eq!(
            erase_gate(None, Some("hv-1")),
            Err(ReleaseBlocker::Open("hv-1".into()))
        );
    }

    #[test]
    fn test_erase_gate_allows_only_a_fully_released_volume() {
        assert_eq!(erase_gate(None, None), Ok(()));
    }

    #[test]
    fn test_release_blocker_names_what_holds_the_volume() {
        assert!(ReleaseBlocker::Mounted("/mnt/encrypted/v".into())
            .to_string()
            .contains("/mnt/encrypted/v"));
        assert!(ReleaseBlocker::Open("hv-1".into())
            .to_string()
            .contains("hv-1"));
    }

    // -- mount table parsing -------------------------------------------------

    #[test]
    fn test_is_mounted_in_matches_a_whole_field() {
        let mounts = "\
sysfs /sys sysfs rw,nosuid 0 0
/dev/mapper/hv-1 /mnt/encrypted/vault ext4 rw,relatime 0 0
/dev/sdb1 /mnt/encrypted/vault2 ext4 rw 0 0
";
        assert!(is_mounted_in(mounts, "/mnt/encrypted/vault"));
        assert!(
            !is_mounted_in(mounts, "/mnt/encrypted/vaul"),
            "a prefix must not match"
        );
        assert!(
            is_mounted_in(mounts, "/mnt/encrypted/vault/"),
            "a trailing slash on the query is tolerated"
        );
    }

    #[test]
    fn test_is_mounted_in_decodes_kernel_octal_escapes() {
        let mounts = "/dev/sdc1 /mnt/encrypted/my\\040vault ext4 rw 0 0\n";
        assert!(is_mounted_in(mounts, "/mnt/encrypted/my vault"));
        assert_eq!(unescape_mount_field("/a\\040b\\011c"), "/a b\tc");
        assert_eq!(
            unescape_mount_field("/a\\04"),
            "/a\\04",
            "a short escape stays as-is"
        );
        assert_eq!(unescape_mount_field("/a\\0zz"), "/a\\0zz");
    }

    #[test]
    fn test_is_mounted_in_is_false_for_an_empty_table() {
        assert!(!is_mounted_in("", "/mnt/encrypted/vault"));
        assert!(!is_mounted_in("garbage\n", "/mnt/encrypted/vault"));
    }

    // -- bounded subprocesses ------------------------------------------------

    #[test]
    fn test_display_command_renders_the_full_argv() {
        assert_eq!(
            display_command("sudo", &["cryptsetup", "close", "hv-1"]),
            "sudo cryptsetup close hv-1"
        );
    }

    #[tokio::test]
    async fn test_run_timed_returns_the_output_of_a_fast_command() {
        let output = run_timed("printf", &["ok"], None, Duration::from_secs(5))
            .await
            .expect("a fast command must return");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "ok");
        assert!(output.status.success());
    }

    #[tokio::test]
    async fn test_run_timed_feeds_stdin_to_the_child() {
        let output = run_timed("cat", &[], Some("secret\n"), Duration::from_secs(5))
            .await
            .expect("cat must echo what it read");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "secret\n");
    }

    #[tokio::test]
    async fn test_run_timed_kills_a_command_that_never_returns() {
        let error = run_timed("sleep", &["30"], None, Duration::from_millis(120))
            .await
            .expect_err("the deadline must fire");
        match error {
            EncryptionError::Timeout { command, secs } => {
                assert!(command.contains("sleep"), "{command}");
                assert_eq!(secs, 0, "120ms rounds down to 0 whole seconds");
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_ensure_success_keeps_the_tool_stderr() {
        let output = run_timed(
            "sh",
            &["-c", "echo 'device is busy' >&2; exit 1"],
            None,
            Duration::from_secs(5),
        )
        .await
        .expect("sh ran");
        match ensure_success(output) {
            Err(EncryptionError::CommandFailed(stderr)) => assert_eq!(stderr, "device is busy"),
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }
}
