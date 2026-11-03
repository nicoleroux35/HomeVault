/* =============================================================================
 * File          : iso_mount.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Read-only ISO/IMG mounting (phase B2): attach a disc image to a loop
 *   device and expose its filesystem under one directory of the appliance, so
 *   its contents can be browsed in the Explorer like any other folder.
 *
 *   Everything that decides *whether* a mount is allowed — the image must be a
 *   real file under an allowed root, the image type must be one we know how to
 *   mount, the mount point must live under the appliance-managed ISO root, and
 *   a second mount of the same image must reuse the first rather than stack a
 *   second loop device — is pure and unit-tested. Only `attach`, `detach` and
 *   `list_mounts` touch the host, through `sudo losetup`/`sudo mount` with
 *   bounded deadlines.
 *
 * Dependencies:
 *   - serde (>= 1.0), crate::utils::process
 *
 * Usage:
 *   handlers::iso_mount exposes the endpoints; see docs/guides/iso-mount.md.
 * =============================================================================
 */

//! Read-only ISO/IMG mounting (see docs/guides/iso-mount.md).
//!
//! An ISO is mounted **read-only** and always under a directory this module
//! owns (`{DATA_DIRECTORY}/iso/<slug>`), never at an arbitrary path: an ISO is
//! a filesystem image, and mounting it elsewhere would let an admin overlay a
//! path the rest of the appliance believes it controls. Nothing is ever
//! extracted — the image is attached to a loop device, which is why the
//! content is available instantly however large the image is.
//!
//! The feature is opt-in and off until used: no loop device exists until an
//! admin mounts something.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Deadline for `losetup`. Attaching a local file is a pair of syscalls; a
/// `losetup` that has not answered in ten seconds is stuck on a dead device.
pub const LOSETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for `mount`/`umount`. A read-only local mount settles in
/// milliseconds; a partition table that needs probing is still far below this.
pub const MOUNT_TIMEOUT: Duration = Duration::from_secs(20);

/// Deadline for a read-only capability probe (`which losetup`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The directory name an ISO is mounted under. Fixed, so the mount point is
/// always `<iso_root>/<slug>` and never derivable from a client path.
pub const ISO_DIR_NAME: &str = "iso";

/// Image extensions this module will mount.
///
/// An allowlist rather than a blocklist: `mount -o loop` will happily treat
/// many things as a filesystem, and the whole point is that only a disc image
/// goes through this path.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["iso", "img"];

/// One mounted image, as the API reports it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IsoMount {
    /// Stable id for this mount (`slug` of the image file name).
    pub id: String,
    /// Absolute path of the image that was mounted.
    pub image_path: String,
    /// Human name shown in the Explorer (`<file name>`).
    pub label: String,
    /// Directory the contents are reachable at.
    pub mount_point: String,
    /// The loop device backing it, when known (`/dev/loop3`).
    pub loop_device: Option<String>,
    /// Size of the image in bytes.
    pub size_bytes: u64,
}

/// Why a mount request was refused before anything touched the host.
///
/// Each variant is a decision, not a guess — the handler turns it into the
/// status from `docs/API.md`'s capability contract (`400` for a request the
/// server refuses on its merits, `503` for a missing tool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsoRefusal {
    /// The path is not under a root the caller may read.
    OutsideRoots,
    /// The path exists but is not a regular file.
    NotAFile,
    /// The extension is not in [`SUPPORTED_EXTENSIONS`].
    UnsupportedType(String),
    /// The image file does not exist.
    Missing,
    /// A second mount of the same image, or the same mount point, was asked
    /// for with a different image.
    AlreadyMounted(String),
}

impl IsoRefusal {
    /// The sentence a caller sees. It names the rule, never just "invalid".
    pub fn message(&self) -> String {
        match self {
            Self::OutsideRoots => {
                "Access denied: the image is outside the folders you may read".to_string()
            }
            Self::NotAFile => "Only a regular image file can be mounted".to_string(),
            Self::UnsupportedType(ext) => format!(
                "Cannot mount a '.{}' file. Supported: {}",
                ext,
                SUPPORTED_EXTENSIONS
                    .iter()
                    .map(|e| format!(".{e}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Missing => "The image file no longer exists".to_string(),
            Self::AlreadyMounted(what) => format!("'{}' is already mounted", what),
        }
    }
}

/// The extension of `path`, lowercased, or `""` when it has none.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Whether this module is willing to mount `path`, by extension alone.
///
/// A pure predicate so the check is testable without a filesystem — the
/// existence and root checks need one and are separate.
pub fn is_supported_image(path: &Path) -> bool {
    SUPPORTED_EXTENSIONS.contains(&extension_of(path).as_str())
}

/// A filesystem-safe, collision-resistant directory name for an image.
///
/// The slug keeps only `[a-z0-9-]`, collapses runs of separators, and appends
/// a 8-hex-digit hash of the full path. The hash is what makes two images with
/// the same file name in different folders mount side by side instead of one
/// silently replacing the other; the readable prefix is what makes
/// `ls {DATA_DIRECTORY}/iso` answer "what is mounted?" without a database.
///
/// Deterministic: the same path always produces the same slug, so re-mounting
/// an image reuses its directory rather than leaking a new one per attempt.
pub fn slug_for(image_path: &Path) -> String {
    let stem = image_path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stem = stem
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or(stem);

    let mut readable: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while readable.contains("--") {
        readable = readable.replace("--", "-");
    }
    let readable = readable.trim_matches('-');
    // Keep the directory name well under PATH_MAX even for a very long file
    // name; the hash carries the uniqueness, not the prefix.
    let readable: String = readable.chars().take(48).collect();
    let readable = readable.trim_end_matches('-');

    let hash = fnv1a_64(image_path.to_string_lossy().as_bytes());
    if readable.is_empty() {
        format!("{hash:016x}")
    } else {
        format!("{readable}-{hash:016x}")
    }
}

/// FNV-1a over the path bytes. Not a security primitive — it only has to make
/// two different paths produce two different slugs, deterministically.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Why a caller-supplied mount point is refused.
///
/// The mount point is **derived**, never taken from the request: the handler
/// computes `<iso_root>/<slug>` itself. This function exists so the derivation
/// is asserted rather than assumed, and so the "under the ISO root, no `..`,
/// no absolute escape" rule is testable on its own.
pub fn is_inside_iso_root(iso_root: &Path, candidate: &Path) -> bool {
    let Ok(root) = iso_root.canonicalize_like() else {
        return false;
    };
    let Ok(candidate) = candidate.canonicalize_like() else {
        return false;
    };

    // A lexical containment test plus an explicit `..` refusal: canonicalising
    // a not-yet-created mount point cannot resolve a symlink, so the lexical
    // check is what catches `../..` before the directory exists.
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return false;
    }
    candidate.starts_with(&root) && candidate != root
}

/// `canonicalize`, or a lexical normalisation when the path does not exist yet
/// (a mount point is created by `mkdir` just before the mount).
trait CanonicalizeLike {
    fn canonicalize_like(&self) -> Result<PathBuf, ()>;
}

impl CanonicalizeLike for Path {
    fn canonicalize_like(&self) -> Result<PathBuf, ()> {
        if let Ok(resolved) = self.canonicalize() {
            return Ok(resolved);
        }
        let mut out = PathBuf::new();
        for component in self.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        if out.is_absolute() {
            Ok(out)
        } else {
            Err(())
        }
    }
}

/// Decide whether `image` may be mounted, given the roots the caller may read.
///
/// Pure apart from the metadata read that answers "is it a regular file"; the
/// root check takes already-resolved paths so this stays testable without
/// touching the filesystem layout of the host.
pub fn check_mountable(
    image: &Path,
    allowed_roots: &[PathBuf],
    already_mounted: &[IsoMount],
) -> Result<(), IsoRefusal> {
    if !is_supported_image(image) {
        return Err(IsoRefusal::UnsupportedType(extension_of(image)));
    }
    let metadata = match std::fs::metadata(image) {
        Ok(m) => m,
        Err(_) => return Err(IsoRefusal::Missing),
    };
    if !metadata.is_file() {
        return Err(IsoRefusal::NotAFile);
    }

    let image_str = image.to_string_lossy().to_string();
    let inside_a_root = allowed_roots.iter().any(|root| image.starts_with(root));
    if !inside_a_root {
        return Err(IsoRefusal::OutsideRoots);
    }

    if let Some(existing) = already_mounted.iter().find(|m| m.image_path == image_str) {
        return Err(IsoRefusal::AlreadyMounted(existing.label.clone()));
    }
    Ok(())
}

/// The argv that attaches `image` to a loop device, read-only.
///
/// `-r` (read-only) and `--show` (print the device it picked) are the two
/// flags the rest of the code depends on: a writable loop device on a shared
/// image is a data-corruption path, and guessing `/dev/loopN` would race every
/// other process on the host.
pub fn losetup_attach_args(image: &Path) -> Vec<String> {
    vec![
        "losetup".to_string(),
        "--find".to_string(),
        "--show".to_string(),
        "--read-only".to_string(),
        image.to_string_lossy().to_string(),
    ]
}

/// The argv that detaches a loop device. `-d` only, never `--detach-all`: a
/// blanket detach would tear down loop devices this appliance did not create.
pub fn losetup_detach_args(device: &str) -> Vec<String> {
    vec!["losetup".to_string(), "-d".to_string(), device.to_string()]
}

/// The argv that mounts a loop device read-only at `mount_point`.
///
/// Read-only is not a nicety: the image is user data, and a writable mount of
/// an ISO that happens to hold a writable filesystem lets a browse operation
/// mutate the source archive. `nosuid`/`nodev` keep a crafted image from
/// introducing a setuid binary or a device node into the appliance's namespace.
pub fn mount_args(device: &str, mount_point: &Path) -> Vec<String> {
    vec![
        "mount".to_string(),
        "-o".to_string(),
        "loop,ro,nosuid,nodev".to_string(),
        device.to_string(),
        mount_point.to_string_lossy().to_string(),
    ]
}

/// The argv that unmounts a mount point.
pub fn umount_args(mount_point: &Path) -> Vec<String> {
    vec![
        "umount".to_string(),
        mount_point.to_string_lossy().to_string(),
    ]
}

/// A row of `/proc/mounts` that backs an ISO mount point, if any.
///
/// Parsing `/proc/mounts` is what makes `list_mounts` reflect the **host**, not
/// a JSON file that could disagree with it after a reboot or a manual
/// `umount`. Returns `(device, mount_point)` for the first matching line.
pub fn find_mount_point(mounts: &str, mount_point: &Path) -> Option<(String, String)> {
    let wanted = mount_point.to_string_lossy();
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let Some(device) = it.next() else { continue };
        let Some(point) = it.next() else { continue };
        if point == wanted {
            return Some((device.to_string(), point.to_string()));
        }
    }
    None
}

/// Every ISO mount point under `iso_root` that `/proc/mounts` confirms.
///
/// The filesystem is the authority: a directory with no mount on it is not a
/// mount, and reporting it as one would let the Explorer show an empty folder
/// while claiming an image is available.
pub fn mounts_under<'a>(mounts: &str, iso_root: &Path, known: &'a [IsoMount]) -> Vec<&'a IsoMount> {
    known
        .iter()
        .filter(|m| find_mount_point(mounts, Path::new(&m.mount_point)).is_some())
        .filter(|m| Path::new(&m.mount_point).starts_with(iso_root))
        .collect()
}

/// Whether the `losetup`/`mount` pair this module needs is installed.
///
/// `503` (not `400`) when it is not — see `docs/API.md`'s capability contract.
pub fn is_available() -> bool {
    ["losetup", "mount", "umount"].iter().all(|tool| {
        run_timed(std::process::Command::new("which").arg(tool), PROBE_TIMEOUT)
            .map(|out| out.status.success())
            .unwrap_or(false)
    })
}

/// Run one `sudo` argv under a deadline and turn a non-zero exit into the
/// tool's own stderr, so the caller sees why rather than "the mount failed".
pub fn run_sudo(args: &[String], deadline: Duration) -> Result<String, String> {
    let Some((program, rest)) = args.split_first() else {
        return Err("empty command".to_string());
    };
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(program).args(rest);

    let output = run_timed(&mut cmd, deadline).map_err(|e| {
        format!(
            "{} did not answer within {}s: {}",
            program,
            deadline.as_secs(),
            e
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{} failed with {}", program, output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(id: &str, image: &str, point: &str) -> IsoMount {
        IsoMount {
            id: id.to_string(),
            image_path: image.to_string(),
            label: id.to_string(),
            mount_point: point.to_string(),
            loop_device: Some("/dev/loop0".to_string()),
            size_bytes: 1024,
        }
    }

    // -- extension allowlist ------------------------------------------------

    #[test]
    fn supported_extensions_are_iso_and_img_only() {
        assert!(is_supported_image(Path::new("/x/ubuntu.iso")));
        assert!(is_supported_image(Path::new("/x/disk.IMG")));
        // Not a disc image: mounting these is not this feature's job.
        for other in ["/x/a.dmg", "/x/a.zip", "/x/a.tar", "/x/a.exe", "/x/noext"] {
            assert!(!is_supported_image(Path::new(other)), "{other}");
        }
    }

    #[test]
    fn unsupported_type_names_the_allowed_set() {
        let err = IsoRefusal::UnsupportedType("dmg".to_string()).message();
        assert!(err.contains(".dmg"));
        assert!(err.contains(".iso"));
        assert!(err.contains(".img"));
    }

    // -- slug ---------------------------------------------------------------

    #[test]
    fn slug_is_deterministic_and_readable() {
        let a = slug_for(Path::new("/data/isos/Ubuntu 24.04.iso"));
        let b = slug_for(Path::new("/data/isos/Ubuntu 24.04.iso"));
        assert_eq!(a, b, "the same path must mount into the same directory");
        assert!(a.starts_with("ubuntu-24-04-"), "{a}");
        assert!(!a.contains(' '), "{a}");
        assert!(!a.contains('/'), "{a}");
    }

    #[test]
    fn same_file_name_in_two_folders_gets_two_slugs() {
        // Without the hash, the second mount would silently take over the
        // first mount point.
        let a = slug_for(Path::new("/data/a/disk.iso"));
        let b = slug_for(Path::new("/data/b/disk.iso"));
        assert_ne!(a, b);
        assert!(a.starts_with("disk-") && b.starts_with("disk-"));
    }

    #[test]
    fn slug_survives_a_name_with_nothing_usable() {
        let slug = slug_for(Path::new("/data/•••.iso"));
        // Still a valid directory name, even though the readable part vanished.
        assert!(!slug.is_empty());
        assert!(slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn slug_is_bounded_for_a_very_long_name() {
        let long = format!("/data/{}.iso", "x".repeat(300));
        assert!(slug_for(Path::new(&long)).chars().count() <= 48 + 1 + 16);
    }

    // -- mount-point containment -------------------------------------------

    #[test]
    fn mount_point_must_stay_under_the_iso_root() {
        let root = Path::new("/var/lib/homevault/iso");
        assert!(is_inside_iso_root(
            root,
            Path::new("/var/lib/homevault/iso/ubuntu-abc")
        ));
        // The root itself is not a mount point — mounting there would overlay
        // the directory every other mount lives in.
        assert!(!is_inside_iso_root(root, root));
        // A sibling that merely shares the prefix is not inside.
        assert!(!is_inside_iso_root(
            root,
            Path::new("/var/lib/homevault/isofoo/x")
        ));
        // An escape is refused even though it lexically starts with the root.
        assert!(!is_inside_iso_root(
            root,
            Path::new("/var/lib/homevault/iso/../../etc")
        ));
        assert!(!is_inside_iso_root(root, Path::new("/etc")));
    }

    // -- pre-flight decisions ----------------------------------------------

    #[test]
    fn a_type_outside_the_allowlist_is_refused_before_any_syscall() {
        let err = check_mountable(Path::new("/data/a.zip"), &[PathBuf::from("/data")], &[]);
        assert_eq!(err, Err(IsoRefusal::UnsupportedType("zip".to_string())));
    }

    #[test]
    fn a_missing_image_is_refused() {
        let err = check_mountable(
            Path::new("/nonexistent/missing.iso"),
            &[PathBuf::from("/nonexistent")],
            &[],
        );
        assert_eq!(err, Err(IsoRefusal::Missing));
    }

    #[test]
    fn a_directory_named_iso_is_not_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("folder.iso");
        std::fs::create_dir(&fake).unwrap();
        let err = check_mountable(&fake, &[dir.path().to_path_buf()], &[]);
        assert_eq!(err, Err(IsoRefusal::NotAFile));
    }

    #[test]
    fn an_image_outside_every_allowed_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("ubuntu.iso");
        std::fs::write(&image, b"not really an iso").unwrap();
        let err = check_mountable(&image, &[PathBuf::from("/some/other/root")], &[]);
        assert_eq!(err, Err(IsoRefusal::OutsideRoots));
    }

    #[test]
    fn a_valid_image_inside_a_root_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("ubuntu.iso");
        std::fs::write(&image, b"x").unwrap();
        assert_eq!(
            check_mountable(&image, &[dir.path().to_path_buf()], &[]),
            Ok(())
        );
    }

    #[test]
    fn the_same_image_is_not_mounted_twice() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("ubuntu.iso");
        std::fs::write(&image, b"x").unwrap();
        let existing = vec![mount(
            "ubuntu",
            &image.to_string_lossy(),
            "/var/lib/homevault/iso/ubuntu-x",
        )];
        let err = check_mountable(&image, &[dir.path().to_path_buf()], &existing);
        assert_eq!(err, Err(IsoRefusal::AlreadyMounted("ubuntu".to_string())));
    }

    // -- argv generation ----------------------------------------------------

    #[test]
    fn attach_is_read_only_and_prints_the_device() {
        let args = losetup_attach_args(Path::new("/data/a.iso"));
        assert_eq!(args[0], "losetup");
        assert!(args.contains(&"--read-only".to_string()));
        assert!(args.contains(&"--show".to_string()));
        assert_eq!(args.last().unwrap(), "/data/a.iso");
    }

    #[test]
    fn detach_names_one_device_only() {
        let args = losetup_detach_args("/dev/loop3");
        // A blanket `--detach-all` would tear down loop devices this appliance
        // never created.
        assert!(!args.iter().any(|a| a.contains("detach-all")));
        assert_eq!(args, vec!["losetup", "-d", "/dev/loop3"]);
    }

    #[test]
    fn mount_is_read_only_nosuid_and_nodev() {
        let args = mount_args("/dev/loop3", Path::new("/var/lib/homevault/iso/x"));
        let opts = &args[2];
        for required in ["ro", "nosuid", "nodev", "loop"] {
            assert!(opts.contains(required), "{opts} must carry {required}");
        }
        assert!(
            !opts.contains("rw"),
            "a writable mount must never be produced"
        );
    }

    #[test]
    fn umount_names_the_mount_point() {
        assert_eq!(
            umount_args(Path::new("/var/lib/homevault/iso/x")),
            vec!["umount", "/var/lib/homevault/iso/x"]
        );
    }

    // -- /proc/mounts parsing ----------------------------------------------

    const MOUNTS: &str = concat!(
        "/dev/sda1 / ext4 rw,relatime 0 0\n",
        "/dev/loop0 /var/lib/homevault/iso/ubuntu-abc iso9660 ro,nosuid,nodev 0 0\n",
        "/dev/sdb1 /media/usb vfat rw,relatime 0 0\n",
    );

    #[test]
    fn find_mount_point_matches_the_exact_point() {
        let found = find_mount_point(MOUNTS, Path::new("/var/lib/homevault/iso/ubuntu-abc"));
        assert_eq!(
            found,
            Some((
                "/dev/loop0".to_string(),
                "/var/lib/homevault/iso/ubuntu-abc".to_string()
            ))
        );
        assert_eq!(
            find_mount_point(MOUNTS, Path::new("/var/lib/homevault/iso/ubuntu-ab")),
            None,
            "a prefix is not a mount point"
        );
    }

    #[test]
    fn only_mount_points_the_host_confirms_are_reported() {
        let root = Path::new("/var/lib/homevault/iso");
        let known = vec![
            mount("a", "/data/a.iso", "/var/lib/homevault/iso/a-1"),
            // Recorded but not actually mounted: the filesystem is the
            // authority, so this must not be reported as available.
            mount("b", "/data/b.iso", "/var/lib/homevault/iso/b-2"),
        ];
        let mounts = "/dev/loop0 /var/lib/homevault/iso/a-1 iso9660 ro 0 0\n";
        let live = mounts_under(mounts, root, &known);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, "a");
    }

    #[test]
    fn a_mount_outside_the_isolated_root_is_ignored() {
        let root = Path::new("/var/lib/homevault/iso");
        let known = vec![mount("evil", "/data/e.iso", "/mnt/somewhere-else")];
        let mounts = "/dev/loop9 /mnt/somewhere-else iso9660 ro 0 0\n";
        assert!(mounts_under(mounts, root, &known).is_empty());
    }
}
