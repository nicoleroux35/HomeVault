/* =============================================================================
 * File          : recovery_hardware_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Opt-in hardware acceptance for P0-7: corrupt a loop-backed Btrfs primary
 *   superblock, drive `storage::recovery::recover_one`, and prove the
 *   production recovery ladder restores the backup superblock and mounts the
 *   filesystem read-only.
 *
 * Prerequisites (disposable host, root):
 *   - `losetup`, `mkfs.btrfs`, `btrfs`, `mount`, `umount`, `dd`, `findmnt`;
 *   - a kernel with loop devices and Btrfs.
 *
 * Usage:
 *   sudo HV_RECOVERY_HW=1 cargo test --test recovery_hardware_integration -- --nocapture
 *
 *   The suite is skipped by default and always prints why: a skip must never
 *   look like a pass. It never edits `/etc/fstab`; it calls the same
 *   `recover_one` the boot pass uses.
 * =============================================================================
 */

use std::path::PathBuf;
use std::process::Command;

use homevault::storage::recovery::{recover_one, FstabEntry};

const GATE_ENV: &str = "HV_RECOVERY_HW";

fn gate() -> Option<String> {
    if std::env::var(GATE_ENV).as_deref() != Ok("1") {
        return Some(format!(
            "set {GATE_ENV}=1 to run the recovery hardware suite (it creates a loop-backed Btrfs filesystem)"
        ));
    }
    if !is_root() {
        return Some("the recovery hardware suite must run as root (loop device + mount)".into());
    }
    for binary in [
        "losetup",
        "mkfs.btrfs",
        "btrfs",
        "mount",
        "umount",
        "dd",
        "findmnt",
        "sudo",
    ] {
        if !binary_on_path(binary) {
            return Some(format!("`{binary}` is not on PATH"));
        }
    }
    None
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

fn binary_on_path(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(binary).is_file()))
        .unwrap_or(false)
}

fn run(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|e| format!("could not run {:?}: {e}", command))?;
    if !output.status.success() {
        return Err(format!(
            "{:?} failed ({}): {}",
            command,
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Owns the loop device and mount point; Drop always tries to unmount and
/// detach so a failed assertion does not wedge the disposable host.
struct Harness {
    root: PathBuf,
    loop_device: Option<String>,
    mounted: bool,
}

impl Harness {
    fn new() -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!(
            "hv-recovery-hw-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
        let image = root.join("disk.img");
        let file = std::fs::File::create(&image).map_err(|e| format!("create image: {e}"))?;
        file.set_len(512 * 1024 * 1024)
            .map_err(|e| format!("size image: {e}"))?;
        drop(file);

        let loop_device = run(Command::new("losetup").args([
            "-f",
            "--show",
            image.to_str().ok_or("image path is not UTF-8")?,
        ]))?;
        let harness = Self {
            root,
            loop_device: Some(loop_device),
            mounted: false,
        };
        run(Command::new("mkfs.btrfs")
            .args(["-f"])
            .arg(harness.loop_device.as_ref().unwrap()))?;
        Ok(harness)
    }

    fn device(&self) -> &str {
        self.loop_device.as_deref().expect("loop device")
    }

    fn mount_point(&self) -> PathBuf {
        self.root.join("mnt")
    }

    fn corrupt_primary_superblock(&self) -> Result<(), String> {
        // Btrfs writes its primary superblock at 64 KiB. Zeroing that 4 KiB
        // block forces `btrfs rescue super-recover` to use a backup copy.
        run(Command::new("dd")
            .args(["if=/dev/zero", "of="])
            .arg(self.device())
            .args(["bs=4096", "seek=16", "count=1", "conv=notrunc"]))?;
        Ok(())
    }

    fn mount_point_is_ro(&self) -> Result<bool, String> {
        let options = run(Command::new("findmnt")
            .args(["-n", "-o", "OPTIONS", "--target"])
            .arg(self.mount_point()))?;
        Ok(options.split(',').any(|option| option == "ro"))
    }

    fn unmount(&mut self) {
        if self.mounted {
            let _ = Command::new("umount").arg(self.mount_point()).output();
            self.mounted = false;
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.unmount();
        if let Some(device) = self.loop_device.take() {
            let _ = Command::new("losetup").args(["-d", &device]).output();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn btrfs_corrupt_primary_superblock_mounts_read_only() {
    let Some(reason) = gate() else {
        let mut harness = Harness::new().expect("create loop-backed Btrfs");
        std::fs::create_dir_all(harness.mount_point()).expect("mount point");
        harness
            .corrupt_primary_superblock()
            .expect("corrupt primary superblock");

        let entry = FstabEntry {
            marker: "# homevault-pool:recovery-hw".to_string(),
            source: harness.device().to_string(),
            mount_point: harness.mount_point().to_string_lossy().to_string(),
            fstype: "btrfs".to_string(),
            options: format!(
                "defaults,nofail,x-systemd.device-timeout=30s,x-systemd.mount-timeout=30s"
            ),
        };
        let status = recover_one(&entry).await.expect("recovery ladder");
        assert_eq!(
            status, "mounted-ro",
            "a corrupt primary superblock must recover into a read-only mount"
        );
        harness.mounted = true;
        assert!(
            harness.mount_point_is_ro().expect("read mount options"),
            "the recovered mount must carry the ro option"
        );
        harness.unmount();
        return;
    };
    eprintln!("SKIPPED btrfs_corrupt_primary_superblock_mounts_read_only: {reason}");
}
