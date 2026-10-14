/* =============================================================================
 * File          : encryption.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   LUKS2 disk encryption for standalone, unassigned block devices: create
 *   (format + passphrase), open/close (mapper management) and destroy.
 *   Deliberately scoped to single devices — not RAID arrays or ZFS pools,
 *   which have their own native encryption paths.
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

use std::path::Path;
use std::process::Stdio;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// Marker comment prefix used to make fstab edits idempotent and reversible,
/// mirroring the technique used elsewhere in the codebase for RAID mounts —
/// written locally here (not reused) since that helper lives in a frozen file.
fn fstab_marker(label: &str) -> String {
    format!("# homevault-encrypted:{}", label)
}

// Every call in this file goes through `sudo` — `cryptsetup`/`mount`/
// `umount` on a real block device need root regardless of the calling
// process's own group memberships (confirmed live against a device given
// realistic `660 root:disk` permissions, not the world-writable default a
// freshly-created loop device gets), same reasoning as the mdadm and
// ZFS/Btrfs pool fixes elsewhere in this codebase. `sudo` transparently
// forwards stdin, so the passphrase-via-stdin design above is unaffected.
async fn run_with_stdin(cmd: &str, args: &[&str], stdin_data: &str) -> Result<(), EncryptionError> {
    let mut child = Command::new("sudo")
        .arg(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_data.as_bytes()).await?;
        stdin.flush().await?;
    }

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EncryptionError::CommandFailed(stderr.trim().to_string()));
    }
    Ok(())
}

/// Formats `device` as a new LUKS2 container with `passphrase`, then opens
/// it under `mapper_name` (available afterwards at `/dev/mapper/<mapper_name>`).
pub async fn format_and_open(
    device: &str,
    mapper_name: &str,
    passphrase: &str,
) -> Result<(), EncryptionError> {
    run_with_stdin(
        "cryptsetup",
        &["luksFormat", "--type", "luks2", "--batch-mode", device, "-"],
        passphrase,
    )
    .await?;

    run_with_stdin(
        "cryptsetup",
        &["open", device, mapper_name, "--key-file", "-"],
        passphrase,
    )
    .await
}

/// Unlocks an already-formatted LUKS device under `mapper_name`.
pub async fn open(
    device: &str,
    mapper_name: &str,
    passphrase: &str,
) -> Result<(), EncryptionError> {
    run_with_stdin(
        "cryptsetup",
        &["open", device, mapper_name, "--key-file", "-"],
        passphrase,
    )
    .await
}

/// Locks (closes) a mapper device. Idempotent — succeeds even if already closed.
pub async fn close(mapper_name: &str) -> Result<(), EncryptionError> {
    let output = Command::new("sudo")
        .arg("cryptsetup")
        .args(["close", mapper_name])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Already closed / not active is not a real error for a "lock" action.
        if stderr.contains("not active") {
            return Ok(());
        }
        return Err(EncryptionError::CommandFailed(stderr.trim().to_string()));
    }
    Ok(())
}

/// Returns true if `mapper_name` is currently open/unlocked.
pub async fn is_open(mapper_name: &str) -> bool {
    Command::new("sudo")
        .arg("cryptsetup")
        .args(["status", mapper_name])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns the LUKS UUID of a formatted device.
pub async fn luks_uuid(device: &str) -> Result<String, EncryptionError> {
    let output = Command::new("sudo")
        .arg("cryptsetup")
        .args(["luksUUID", device])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EncryptionError::CommandFailed(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Mounts the (already-unlocked) mapper device at `mount_point`.
pub async fn mount(mapper_name: &str, mount_point: &str) -> Result<(), EncryptionError> {
    // `sudo mkdir`, not `std::fs::create_dir_all` — this runs under
    // ENCRYPTED_MOUNT_BASE, root-owned, and sudo only wraps external Command
    // invocations, not Rust's own syscalls (same fix as pools.rs).
    let mkdir_output = Command::new("sudo")
        .arg("mkdir")
        .args(["-p", mount_point])
        .output()
        .await?;
    if !mkdir_output.status.success() {
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        return Err(EncryptionError::CommandFailed(format!(
            "Failed to create mount point: {}",
            stderr.trim()
        )));
    }
    let output = Command::new("sudo")
        .arg("mount")
        .args([&format!("/dev/mapper/{}", mapper_name), mount_point])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EncryptionError::CommandFailed(stderr.trim().to_string()));
    }
    Ok(())
}

/// Unmounts a mount point. Idempotent — succeeds even if not mounted.
pub async fn unmount(mount_point: &str) -> Result<(), EncryptionError> {
    let output = Command::new("sudo")
        .arg("umount")
        .arg(mount_point)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not mounted") {
            return Ok(());
        }
        return Err(EncryptionError::CommandFailed(stderr.trim().to_string()));
    }
    Ok(())
}

/// Changes an existing volume's passphrase.
pub async fn change_passphrase(
    device: &str,
    old_passphrase: &str,
    new_passphrase: &str,
) -> Result<(), EncryptionError> {
    // cryptsetup luksChangeKey reads the old passphrase then the new one,
    // each as a newline-terminated line on stdin.
    let combined = format!("{}\n{}\n", old_passphrase, new_passphrase);
    run_with_stdin("cryptsetup", &["luksChangeKey", device], &combined).await
}

/// Permanently destroys a LUKS volume's key slots, making its data
/// unrecoverable. Callers must lock the volume first if it's open.
pub async fn erase(device: &str) -> Result<(), EncryptionError> {
    let output = Command::new("sudo")
        .arg("cryptsetup")
        .args(["erase", "--batch-mode", device])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EncryptionError::CommandFailed(stderr.trim().to_string()));
    }
    // `cryptsetup erase` wipes the key slots (data is now unrecoverable) but
    // leaves the LUKS2 header structure — so `blkid`/lsblk still report
    // `crypto_LUKS` and `DiskService::get_available_devices` keeps treating
    // the device as in-use, meaning the admin can never re-encrypt or
    // repurpose it through HomeVault. Clear the signature so it returns to
    // the available pool. Best-effort: the security-critical step (key-slot
    // erase) already succeeded above.
    let _ = Command::new("sudo")
        .arg("wipefs")
        .args(["-a", device])
        .output()
        .await;
    Ok(())
}

/// Writes `content` to `path` via `sudo tee`, not a direct `std::fs` write —
/// `/etc/fstab` is `644 root:root`, so a direct write from the unprivileged
/// homevault process fails with EACCES (same fix as pools.rs's identical
/// helper — this file doesn't reuse that one since it lives in a frozen
/// file, per the doc comment on `fstab_marker` above).
fn write_file_as_root(path: &str, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("sudo")
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(content.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(())
}

/// Adds a `noauto` fstab entry (marked so it can be cleanly removed later)
/// pointing at the mapper device. `noauto` is deliberate: the volume must
/// never be auto-mounted at boot since it's still locked at that point.
pub fn append_fstab_entry(
    mapper_name: &str,
    mount_point: &str,
    filesystem: &str,
    label: &str,
) -> Result<(), EncryptionError> {
    let marker = fstab_marker(label);
    let existing = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    if existing.contains(&marker) {
        return Ok(());
    }
    let line = format!(
        "{}\n/dev/mapper/{} {} {} noauto,defaults 0 0\n",
        marker, mapper_name, mount_point, filesystem
    );
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&line);
    write_file_as_root("/etc/fstab", &updated)?;
    Ok(())
}

/// Removes a previously-added marked fstab entry (marker line + the line
/// immediately after it), if present.
pub fn remove_fstab_entry(label: &str) -> Result<(), EncryptionError> {
    let marker = fstab_marker(label);
    let path = "/etc/fstab";
    if !Path::new(path).exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut skip_next = false;
    for line in lines {
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
    let mut new_content = out.join("\n");
    if !new_content.is_empty() {
        new_content.push('\n');
    }
    write_file_as_root(path, &new_content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! `encryption.rs` is a thin wrapper around `cryptsetup`/`mount` (all
    //! side effects, all root — covered by `storage_integration.rs` at the
    //! validation level and by the opt-in real-LUKS lifecycle in
    //! `tests/e2e/storage/storage.spec.ts`). The only pure logic here is the
    //! fstab marker, which `append_fstab_entry` writes and `remove_fstab_entry`
    //! matches on — if the two ever disagree, a deleted volume's fstab line
    //! is orphaned, so pin the format.
    use super::*;

    #[test]
    fn test_fstab_marker_format_is_stable() {
        assert_eq!(fstab_marker("photos"), "# homevault-encrypted:photos");
        assert_eq!(fstab_marker("vault-01"), "# homevault-encrypted:vault-01");
    }

    #[test]
    fn test_fstab_marker_is_distinct_per_label() {
        assert_ne!(fstab_marker("a"), fstab_marker("b"));
    }
}
