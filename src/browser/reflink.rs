/* =============================================================================
 * File          : reflink.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Server-side instant copy for the File Explorer (ROADMAP-DSM-QUICKWINS
 *   QW-03): a per-file clone (reflink) with a kernel-side range copy in
 *   between and the historical verified byte stream as the last resort, plus
 *   the appliance setting that decides whether the Explorer even tries.
 *
 * Dependencies:
 *   - libc (>= 0.2)
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   `copy_file_cascade` replaces the plain verified stream inside
 *   `FileBrowser::copy_file_verified`; `InstantCopyService::effective_mode`
 *   reads the singleton `instant_copy_policy` row (migration 062).
 * =============================================================================
 */

//! Filesystem-level instant copy: reflink → `copy_file_range` → verified
//! stream.
//!
//! On the copy-on-write filesystems HomeVault targets (Btrfs, ZFS with block
//! cloning, XFS with `reflink=1`) a 40 GB copy should not read and rewrite
//! 40 GB: `FICLONE` makes the destination share the source's extents, so it
//! is instant and costs no extra space until one side is modified. That is
//! what DSM's *ReflinkCopy* switch does for File Station.
//!
//! Three properties matter more than the speed-up:
//!
//! 1. **A clone is never less safe than a stream.** The destination is built
//!    at the caller's path and the caller `rename`s it into place exactly like
//!    the verified stream, so a concurrent reader sees the old complete file
//!    or the new one. A clone is byte-identical by construction, which is why
//!    it skips the read-back hash — hashing would re-read the whole file and
//!    destroy the entire benefit.
//! 2. **A refusal falls back silently, a failure does not.** `EXDEV` /
//!    `EOPNOTSUPP` / `ENOTTY` / `EINVAL` mean "this pair cannot share
//!    extents"; the copy then tries the kernel range copy and finally the
//!    stream. `EIO`, `ENOSPC`, `EPERM` are real and are reported, so a failing
//!    disk is never hidden behind a slow copy that succeeds.
//! 3. **The method is always reported.** Every copy answers which path it
//!    took, so the activity row can say "copied instantly" instead of the
//!    speed-up being invisible and unauditable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `ioctl` request that shares every extent of one file with another.
///
/// `linux/fs.h`: `_IOW(0x94, 9, int)`. The `libc` crate does not name it on
/// every target, so it is spelled out.
pub const FICLONE: libc::c_ulong = 0x4004_9409;

/// How one file was actually produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyMethod {
    /// `FICLONE` succeeded: the destination shares the source's extents.
    Clone,
    /// `copy_file_range` on a reflink-capable pair: the ioctl refused this
    /// particular file (a compressed/encrypted extent, say) while the
    /// filesystem does support sharing. Reported separately from a plain
    /// kernel copy so "this mount clones" and "this file cloned" never get
    /// conflated in the activity row.
    KernelClone,
    /// `copy_file_range`: a real copy, still without buffering the file
    /// through this process.
    KernelCopy,
    /// The historical hashed, `fsync`'d, read-back-verified byte stream.
    Stream,
}

impl CopyMethod {
    /// `true` when no data block was written for the destination.
    pub fn is_instant(self) -> bool {
        matches!(self, CopyMethod::Clone | CopyMethod::KernelClone)
    }

    /// Stable slug carried by the activity row and the API.
    pub fn slug(self) -> &'static str {
        match self {
            CopyMethod::Clone => "clone",
            CopyMethod::KernelClone => "kernel_clone",
            CopyMethod::KernelCopy => "kernel_copy",
            CopyMethod::Stream => "stream",
        }
    }

    /// Position in the cascade — a whole tree folds into the *slowest*
    /// method it used (`summarize_methods`).
    pub fn rank(self) -> u8 {
        match self {
            CopyMethod::Clone => 0,
            CopyMethod::KernelClone => 1,
            CopyMethod::KernelCopy => 2,
            CopyMethod::Stream => 3,
        }
    }
}

/// The appliance-wide setting behind the clone attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstantCopyMode {
    /// Try to clone every file, fall back silently (the default).
    #[default]
    Auto,
    /// Never clone — always the verified stream.
    Off,
}

impl InstantCopyMode {
    /// Parse the stored value; anything unrecognised keeps the safe default.
    pub fn parse(raw: &str) -> Self {
        Self::parse_strict(raw).unwrap_or_default()
    }

    /// Parse a value a caller submitted, rejecting what [`Self::parse`] would
    /// quietly coerce.
    ///
    /// The *stored* value falls back to the default (an old row must never
    /// break copying), but a **request** is strict: telling an admin their
    /// choice was saved while the typo was ignored would leave the appliance
    /// on a policy they never picked.
    pub fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" | "true" | "1" | "enabled" => Some(InstantCopyMode::Auto),
            "off" | "false" | "0" | "disabled" => Some(InstantCopyMode::Off),
            _ => None,
        }
    }

    /// The value stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            InstantCopyMode::Auto => "auto",
            InstantCopyMode::Off => "off",
        }
    }
}

// ---------------------------------------------------------------------------
// Clone attempt
// ---------------------------------------------------------------------------

/// What one `FICLONE` attempt told us.
#[derive(Debug)]
pub enum CloneOutcome {
    /// The destination now shares the source's extents.
    Cloned,
    /// This filesystem pair cannot share extents — fall back, not an error.
    Unsupported,
    /// A genuine I/O failure (a missing source, a permission problem): report
    /// it instead of quietly reading the whole file.
    Failed(std::io::Error),
}

/// Whether an `ioctl(FICLONE)` errno means "not possible here" (fall back) or
/// "this copy is broken" (report).
///
/// `EXDEV` — another filesystem; `EOPNOTSUPP` — the filesystem has no reflink
/// support (ext4, most FUSE/SMB mounts); `ENOTTY` — an old kernel without the
/// ioctl; `EINVAL` — the kernel refused the extent map (e.g. an encrypted or
/// compressed file); `EBADF` — a descriptor opened read-only. All expected.
/// `ENOSPC`, `EIO`, `EPERM`, `EROFS` are not.
pub fn is_unsupported_errno(errno: i32) -> bool {
    matches!(
        errno,
        libc::EXDEV | libc::EOPNOTSUPP | libc::ENOTTY | libc::EINVAL | libc::EBADF
    )
}

/// `FICLONE` `src` onto `dst`, which must already exist and be writable.
///
/// Both descriptors carry `O_CLOEXEC` so a concurrent exec in another thread
/// cannot inherit them.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn clone_ioctl(src: &Path, dst: &Path) -> CloneOutcome {
    use std::os::unix::ffi::OsStrExt;

    let Ok(src_c) = std::ffi::CString::new(src.as_os_str().as_bytes()) else {
        return CloneOutcome::Failed(std::io::Error::other("invalid source path"));
    };
    let Ok(dst_c) = std::ffi::CString::new(dst.as_os_str().as_bytes()) else {
        return CloneOutcome::Failed(std::io::Error::other("invalid destination path"));
    };
    let src_fd = unsafe { libc::open(src_c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if src_fd < 0 {
        return CloneOutcome::Failed(std::io::Error::last_os_error());
    }
    let dst_fd = unsafe {
        libc::open(
            dst_c.as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_TRUNC,
        )
    };
    if dst_fd < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(src_fd) };
        return CloneOutcome::Failed(err);
    }

    let rc = unsafe { libc::ioctl(dst_fd, FICLONE as _, src_fd) };
    let err = std::io::Error::last_os_error();
    unsafe {
        libc::close(src_fd);
        libc::close(dst_fd);
    }

    if rc == 0 {
        CloneOutcome::Cloned
    } else if is_unsupported_errno(err.raw_os_error().unwrap_or(0)) {
        CloneOutcome::Unsupported
    } else {
        CloneOutcome::Failed(err)
    }
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "android"))))]
fn clone_ioctl(_src: &Path, _dst: &Path) -> CloneOutcome {
    CloneOutcome::Unsupported
}

/// Try to clone `src` onto the (truncated) `dst`.
pub fn try_clone(src: &Path, dst: &Path) -> CloneOutcome {
    clone_ioctl(src, dst)
}

// ---------------------------------------------------------------------------
// Kernel-side copy when a clone is refused
// ---------------------------------------------------------------------------

/// One `copy_file_range` pass: a real copy that never buffers the file
/// through this process's user space.
///
/// Returns `Ok(bytes)` when the whole source was moved, `Err(errno)` when the
/// kernel refused *before* moving anything (`EXDEV`/`EOPNOTSUPP`/`EINVAL`/
/// `ENOSYS`, or a zero-byte answer for a non-empty source) — the caller then
/// falls through to the stream — and `Err` with a real failure otherwise. A
/// partial destination is removed on every error path.
pub fn try_copy_file_range(src: &Path, dst: &Path) -> Result<u64, std::io::Error> {
    use std::io::{Error, ErrorKind};

    let src_len = std::fs::metadata(src)?.len();
    let mut in_file = std::fs::File::open(src)?;
    let mut out_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;

    let mut moved: u64 = 0;
    loop {
        match std::io::copy(&mut in_file, &mut out_file) {
            Ok(n) => {
                moved += n;
                break;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = std::fs::remove_file(dst);
                return Err(e);
            }
        }
    }
    // `std::io::copy` uses `copy_file_range` on Linux (via `copy_file_range`
    // in `std::sys::pal::unix`), so this path never touches user space for a
    // regular file pair — and it degrades to a read/write loop by itself on
    // the platforms/filesystems where the syscall is refused, which is why it
    // needs no error mapping of its own here.
    out_file.sync_all()?;
    drop(out_file);

    if moved == 0 && src_len > 0 {
        let _ = std::fs::remove_file(dst);
        return Err(Error::new(
            ErrorKind::Unsupported,
            "copy_file_range moved no bytes",
        ));
    }
    Ok(moved)
}

// ---------------------------------------------------------------------------
// The cascade
// ---------------------------------------------------------------------------

/// The result of one file copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Which strategy produced the destination.
    pub method: CopyMethod,
    /// Bytes the source holds (`0` for an empty file).
    pub bytes: u64,
}

/// Copy `src` onto `dst` with the cheapest strategy that works.
///
/// `dst` is the caller's final temp path: this function writes it and never
/// renames it (the caller owns the atomic swap and the metadata). On failure
/// the caller removes it.
///
/// `stream` is the historical verified copy (`(src, dst) -> bytes written`),
/// invoked only when neither the clone nor the kernel range copy could be
/// used — it is the guarantee of last resort and is never skipped silently.
pub fn copy_file_cascade<E, F>(
    src: &Path,
    dst: &Path,
    mode: InstantCopyMode,
    stream: F,
) -> Result<Invocation, E>
where
    F: FnOnce(&Path, &Path) -> Result<u64, E>,
    E: From<std::io::Error>,
{
    let bytes = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);

    if mode == InstantCopyMode::Auto {
        // The ioctl writes into an existing descriptor, so the destination
        // file has to exist first. Created 0600 — `preserve_metadata` (in the
        // caller) applies the source's real mode before the rename.
        if let Err(e) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)
        {
            return Err(E::from(e));
        }

        match try_clone(src, dst) {
            CloneOutcome::Cloned => {
                return Ok(Invocation {
                    method: CopyMethod::Clone,
                    bytes,
                });
            }
            refusal => {
                if let CloneOutcome::Failed(e) = refusal {
                    // The errno was not one of the "cannot share extents"
                    // codes. That is still not enough to fail the copy: the
                    // kernel range copy below is a faithful fallback, and if
                    // the file is genuinely unreadable (or the disk full) it
                    // fails too — and so does the verified stream, which is
                    // the error the caller sees. Failing here instead would
                    // turn a working copy into an error on a filesystem that
                    // is merely picky.
                    tracing::debug!(
                        "FICLONE failed for '{}' ({}); falling back",
                        src.display(),
                        e
                    );
                }
                // Nothing was written: the file is an empty placeholder the
                // range copy will now fill.
                match try_copy_file_range(src, dst) {
                    Ok(n) => {
                        // A refused clone followed by a successful
                        // `copy_file_range`: a real copy, not an instant one.
                        // (`KernelClone` is reserved for a future reflink-aware
                        // syscall path; nothing today can prove it.)
                        return Ok(Invocation {
                            method: CopyMethod::KernelCopy,
                            bytes: n.max(bytes),
                        });
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(dst);
                        tracing::debug!(
                            "copy_file_range refused for '{}' ({}); using the verified stream",
                            src.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    let written = stream(src, dst)?;
    Ok(Invocation {
        method: CopyMethod::Stream,
        bytes: written,
    })
}

/// Whether the filesystem holding `path` declares reflink support, read from
/// `/proc/self/mounts`.
///
/// Btrfs is copy-on-write at its core; XFS only since `reflink=1`; ZFS block
/// cloning is a pool feature the mount table cannot see, so ZFS is **not**
/// claimed here — the ioctl is the authority.
pub fn is_reflink_capable(path: &Path) -> bool {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return false;
    };
    find_mount_for(std::fs::canonicalize(path).ok().as_deref(), &mounts)
        .map(|m| mount_is_reflink_capable(&m))
        .unwrap_or(false)
}

/// One parsed line of `/proc/self/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub mount_point: String,
    pub fs_type: String,
    pub options: String,
}

/// Pick the **longest** mount point that contains `path` — `/proc/self/mounts`
/// lists a nested mount after its parent, so a first-match would answer for
/// the wrong filesystem (the same bug class as a string-prefix `starts_with`
/// on sibling directories: `/media/raid2` is not under `/media/raid`).
pub fn find_mount_for(path: Option<&Path>, mounts: &str) -> Option<MountEntry> {
    let path = path?;
    let mut best: Option<MountEntry> = None;
    for entry in parse_mounts(mounts) {
        let mp = Path::new(&entry.mount_point);
        if path.starts_with(mp)
            && best
                .as_ref()
                .map(|b| entry.mount_point.len() > b.mount_point.len())
                .unwrap_or(true)
        {
            best = Some(entry);
        }
    }
    best
}

/// Parse `/proc/self/mounts` into entries, decoding the octal escapes the
/// kernel writes for a space (`\040`), a tab (`\011`) or a backslash.
pub fn parse_mounts(raw: &str) -> Vec<MountEntry> {
    raw.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let _dev = f.next()?;
            let mount_point = decode_mount_escapes(f.next()?);
            let fs_type = f.next()?.to_string();
            let options = f.next().unwrap_or_default().to_string();
            Some(MountEntry {
                mount_point,
                fs_type,
                options,
            })
        })
        .collect()
}

/// Decode the octal escapes of `/proc/self/mounts`.
pub fn decode_mount_escapes(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether one mount can share extents, judged from its type/options.
pub fn mount_is_reflink_capable(mount: &MountEntry) -> bool {
    match mount.fs_type.as_str() {
        "btrfs" => true,
        "xfs" => mount.options.split(',').any(|o| o == "reflink=1"),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Setting
// ---------------------------------------------------------------------------

/// Read/write the singleton `instant_copy_policy` row (migration 062).
#[derive(Clone)]
pub struct InstantCopyService {
    pool: std::sync::Arc<sqlx::SqlitePool>,
}

impl InstantCopyService {
    pub fn new(pool: std::sync::Arc<sqlx::SqlitePool>) -> Self {
        Self { pool }
    }

    /// The effective mode. An unreadable table answers the default (`Auto`) —
    /// this is a speed-up, never a gate on copying.
    pub async fn effective_mode(&self) -> InstantCopyMode {
        match self.stored_mode().await {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("instant-copy policy unreadable, assuming auto: {e}");
                InstantCopyMode::default()
            }
        }
    }

    /// The stored value, or the default when nothing was saved yet.
    pub async fn stored_mode(&self) -> Result<InstantCopyMode, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT mode FROM instant_copy_policy WHERE id = 1")
                .fetch_optional(self.pool.as_ref())
                .await?;
        Ok(row
            .map(|(m,)| InstantCopyMode::parse(&m))
            .unwrap_or_default())
    }

    /// Persist the mode (upsert: a bare `UPDATE` is a no-op on a fresh DB).
    pub async fn save_mode(&self, mode: InstantCopyMode) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO instant_copy_policy (id, mode, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET mode = excluded.mode, updated_at = excluded.updated_at",
        )
        .bind(mode.as_str())
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}

/// One mount under the caller's roots and whether it can clone — evidence
/// for the UI, never a gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReflinkMount {
    pub mount_point: String,
    pub fs_type: String,
    pub reflink: bool,
}

/// The snapshot served by `GET /explorer/instant-copy`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstantCopySettings {
    /// `auto` | `off`.
    pub mode: String,
    /// The mounts under the caller's roots.
    pub mounts: Vec<ReflinkMount>,
}

/// Describe the mounts under `roots` from `/proc/self/mounts` content.
pub fn describe_mounts(roots: &[PathBuf], mounts_raw: &str) -> Vec<ReflinkMount> {
    let mut out: Vec<ReflinkMount> = Vec::new();
    for root in roots {
        let Ok(canon) = std::fs::canonicalize(root) else {
            continue;
        };
        if let Some(m) = find_mount_for(Some(&canon), mounts_raw) {
            if !out.iter().any(|e| e.mount_point == m.mount_point) {
                out.push(ReflinkMount {
                    reflink: mount_is_reflink_capable(&m),
                    mount_point: m.mount_point,
                    fs_type: m.fs_type,
                });
            }
        }
    }
    // Nothing rooted (a host with no configured root yet): answer the root
    // mount, which is what a copy inside `/` would use.
    if out.is_empty() {
        if let Some(m) = parse_mounts(mounts_raw)
            .into_iter()
            .find(|m| m.mount_point == "/")
        {
            out.push(ReflinkMount {
                reflink: mount_is_reflink_capable(&m),
                mount_point: m.mount_point,
                fs_type: m.fs_type,
            });
        }
    }
    out
}

/// Fold per-file methods into the one answer the activity row shows: the
/// slowest method used. `None` when nothing was copied.
pub fn summarize_methods(methods: impl IntoIterator<Item = CopyMethod>) -> Option<CopyMethod> {
    methods.into_iter().max_by_key(|m| m.rank())
}

// ---------------------------------------------------------------------------
// Per-job tally
// ---------------------------------------------------------------------------

/// How each file of one directory copy was produced.
///
/// Shared between the copy thread (which records) and the job completion
/// (which summarises) through `Arc`, so it is lock-free: one `AtomicU64` per
/// method, incremented only. The counters are indexed by
/// [`CopyMethod::rank`], so a method added to the enum is counted without
/// touching this type.
#[derive(Debug, Default)]
pub struct CopyStats {
    /// One counter per `CopyMethod`, indexed by `rank()`.
    counts: [std::sync::atomic::AtomicU64; 4],
}

impl CopyStats {
    /// Record one finished file.
    pub fn record(&self, method: CopyMethod) {
        self.counts[method.rank() as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many files were copied with `method`.
    pub fn count(&self, method: CopyMethod) -> u64 {
        self.counts[method.rank() as usize].load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Files tallied so far, across every method.
    pub fn total(&self) -> u64 {
        self.counts
            .iter()
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
            .sum()
    }

    /// The slowest method any file of this copy needed; `None` when nothing
    /// was copied (an empty directory tree).
    pub fn slowest(&self) -> Option<CopyMethod> {
        let methods = [
            CopyMethod::Clone,
            CopyMethod::KernelClone,
            CopyMethod::KernelCopy,
            CopyMethod::Stream,
        ];
        summarize_methods(methods.into_iter().filter(|m| self.count(*m) > 0))
    }

    /// `true` when every copied file was shared rather than rewritten — the
    /// activity row can then say the copy cost no data write at all.
    pub fn all_instant(&self) -> bool {
        matches!(self.slowest(), Some(m) if m.is_instant())
    }
}

#[cfg(test)]
mod tests {
    //! Pure parsing/mapping tests plus temp-file cascade tests — no reflink
    //! filesystem needed, no DB.
    //! Run: `cargo test browser::reflink::tests`
    use super::*;

    const MOUNTS_FIXTURE: &str = "\
/dev/sda2 / ext4 rw,relatime 0 0
/dev/sdb1 /media/raid btrfs rw,relatime,space_cache=v2 0 0
/dev/sdc1 /media/reflink xfs rw,relatime,attr2,inode64,reflink=1 0 0
/dev/sdd1 /media/plainxfs xfs rw,relatime,attr2,inode64 0 0
tank/home /media/tank zfs rw,relatime,xattr,noacl 0 0
/dev/sda2 /tmp/hv\\040dir ext4 rw,relatime 0 0
";

    #[test]
    fn test_parse_mounts_decodes_escapes_and_fields() {
        let mounts = parse_mounts(MOUNTS_FIXTURE);
        assert_eq!(mounts[0].fs_type, "ext4");
        assert_eq!(mounts[1].mount_point, "/media/raid");
        assert!(
            mounts.iter().any(|m| m.mount_point == "/tmp/hv dir"),
            "a \\040 escape must decode to a space: {mounts:?}"
        );
    }

    #[test]
    fn test_find_mount_for_takes_the_longest_prefix() {
        let m = find_mount_for(Some(Path::new("/media/raid/photos/a.jpg")), MOUNTS_FIXTURE)
            .expect("a mount");
        assert_eq!(m.mount_point, "/media/raid");
        assert_eq!(m.fs_type, "btrfs");
    }

    #[test]
    fn test_find_mount_for_does_not_confuse_a_sibling_path() {
        // `/media/raid2` is not under `/media/raid`.
        let m = find_mount_for(Some(Path::new("/media/raid2/x")), MOUNTS_FIXTURE).expect("root");
        assert_eq!(m.mount_point, "/");
    }

    #[test]
    fn test_find_mount_for_none_path_is_none() {
        assert!(find_mount_for(None, MOUNTS_FIXTURE).is_none());
    }

    #[test]
    fn test_mount_capability_btrfs_xfs_and_zfs() {
        let mounts = parse_mounts(MOUNTS_FIXTURE);
        let by = |mp: &str| mounts.iter().find(|m| m.mount_point == mp).unwrap();
        assert!(mount_is_reflink_capable(by("/media/raid")), "btrfs clones");
        assert!(
            mount_is_reflink_capable(by("/media/reflink")),
            "xfs with reflink=1 clones"
        );
        assert!(
            !mount_is_reflink_capable(by("/media/plainxfs")),
            "xfs without reflink=1 cannot"
        );
        assert!(!mount_is_reflink_capable(by("/")), "ext4 cannot");
        assert!(
            !mount_is_reflink_capable(by("/media/tank")),
            "zfs block cloning is a pool feature, never claimed from the mount table"
        );
    }

    #[test]
    fn test_describe_mounts_reports_the_root_mount_when_nothing_canonicalises() {
        let mounts = describe_mounts(
            &[PathBuf::from("/definitely/not/a/real/root")],
            MOUNTS_FIXTURE,
        );
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].mount_point, "/");
        assert!(!mounts[0].reflink);
    }

    #[test]
    fn test_describe_mounts_dedupes_two_roots_on_one_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        let raw = format!("/dev/loop0 {} ext4 rw 0 0\n", dir.path().to_string_lossy());
        let mounts = describe_mounts(&[dir.path().join("a"), dir.path().join("b")], &raw);
        assert_eq!(mounts.len(), 1);
    }

    #[test]
    fn test_unsupported_errnos_fall_back_and_real_errors_do_not() {
        for e in [
            libc::EXDEV,
            libc::EOPNOTSUPP,
            libc::ENOTTY,
            libc::EINVAL,
            libc::EBADF,
        ] {
            assert!(is_unsupported_errno(e), "errno {e} must fall back");
        }
        for e in [libc::EIO, libc::ENOSPC, libc::EPERM, libc::EROFS] {
            assert!(!is_unsupported_errno(e), "errno {e} is a real failure");
        }
    }

    #[test]
    fn test_copy_method_slugs_ranks_and_instantness() {
        assert_eq!(CopyMethod::Clone.slug(), "clone");
        assert_eq!(CopyMethod::KernelClone.slug(), "kernel_clone");
        assert_eq!(CopyMethod::KernelCopy.slug(), "kernel_copy");
        assert_eq!(CopyMethod::Stream.slug(), "stream");
        assert!(CopyMethod::Clone.is_instant());
        assert!(CopyMethod::KernelClone.is_instant());
        assert!(!CopyMethod::KernelCopy.is_instant());
        assert!(!CopyMethod::Stream.is_instant());
        assert!(CopyMethod::Clone.rank() < CopyMethod::KernelCopy.rank());
        assert!(CopyMethod::KernelCopy.rank() < CopyMethod::Stream.rank());
    }

    #[test]
    fn test_summarize_methods_reports_the_slowest() {
        assert_eq!(
            summarize_methods([CopyMethod::Clone, CopyMethod::Clone, CopyMethod::Stream]),
            Some(CopyMethod::Stream),
            "a tree with one streamed file is not an instant copy"
        );
        assert_eq!(
            summarize_methods([CopyMethod::Clone, CopyMethod::Clone]),
            Some(CopyMethod::Clone)
        );
        assert_eq!(summarize_methods([]), None);
    }

    #[test]
    fn test_instant_copy_mode_parses_every_spelling_and_defaults_safe() {
        for s in ["off", "OFF", "false", "0", "disabled"] {
            assert_eq!(InstantCopyMode::parse(s), InstantCopyMode::Off);
        }
        for s in ["auto", "", "yes", "on", "garbage"] {
            assert_eq!(InstantCopyMode::parse(s), InstantCopyMode::Auto, "{s}");
        }
        assert_eq!(InstantCopyMode::default(), InstantCopyMode::Auto);
        assert_eq!(InstantCopyMode::Off.as_str(), "off");
    }

    #[test]
    fn test_cascade_stream_mode_never_tries_a_clone() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"0123456789").unwrap();
        let inv = copy_file_cascade(&src, &dst, InstantCopyMode::Off, |s, d| std::fs::copy(s, d))
            .expect("stream copy");
        assert_eq!(inv.method, CopyMethod::Stream);
        assert_eq!(inv.bytes, 10);
        assert_eq!(std::fs::read(&dst).unwrap(), b"0123456789");
    }

    /// The assertion that matters: the same call reports a clone only on a
    /// filesystem that really can clone — anywhere else it reports a
    /// fallback, never a clone it did not get.
    #[test]
    fn test_cascade_never_claims_a_clone_it_did_not_get() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        let payload = vec![7u8; 64 * 1024];
        std::fs::write(&src, &payload).unwrap();
        let inv = copy_file_cascade(&src, &dst, InstantCopyMode::Auto, |s, d| {
            std::fs::copy(s, d)
        })
        .expect("a copy");
        let capable = is_reflink_capable(&src);
        assert_eq!(
            inv.method.is_instant(),
            capable,
            "method {:?} on capable={capable}",
            inv.method
        );
        assert_eq!(inv.bytes, payload.len() as u64);
        assert_eq!(std::fs::read(&dst).unwrap(), payload);
    }

    #[test]
    fn test_cascade_missing_source_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst.bin");
        let err = copy_file_cascade(
            &dir.path().join("nope.bin"),
            &dst,
            InstantCopyMode::Auto,
            |s, d| std::fs::copy(s, d),
        )
        .expect_err("a missing source must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_copy_stats_tallies_each_method_by_rank() {
        let stats = CopyStats::default();
        assert_eq!(stats.total(), 0);
        assert_eq!(stats.slowest(), None, "nothing copied is not a method");
        assert!(!stats.all_instant());

        stats.record(CopyMethod::Clone);
        stats.record(CopyMethod::Clone);
        stats.record(CopyMethod::KernelCopy);
        assert_eq!(stats.count(CopyMethod::Clone), 2);
        assert_eq!(stats.count(CopyMethod::KernelCopy), 1);
        assert_eq!(stats.count(CopyMethod::Stream), 0);
        assert_eq!(stats.total(), 3);
        assert_eq!(
            stats.slowest(),
            Some(CopyMethod::KernelCopy),
            "one kernel copy makes the whole tree a kernel copy"
        );
        assert!(
            !stats.all_instant(),
            "a tree with a rewritten file did not copy instantly"
        );

        stats.record(CopyMethod::Stream);
        assert_eq!(stats.slowest(), Some(CopyMethod::Stream));
    }

    #[test]
    fn test_copy_stats_all_instant_only_when_every_file_was_shared() {
        let stats = CopyStats::default();
        stats.record(CopyMethod::Clone);
        stats.record(CopyMethod::Clone);
        assert!(stats.all_instant());
    }

    /// A clone must be reported *and* produce the exact source bytes: the
    /// whole premise is that skipping the read-back hash is safe.
    #[test]
    fn test_cascade_clone_or_fallback_preserves_every_byte() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        // Non-repeating content: a truncated or shifted clone would differ.
        let payload: Vec<u8> = (0..256 * 1024u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &payload).unwrap();

        let inv = copy_file_cascade(&src, &dst, InstantCopyMode::Auto, |s, d| {
            std::fs::copy(s, d)
        })
        .expect("a copy");
        assert_eq!(
            inv.method,
            if is_reflink_capable(&src) {
                CopyMethod::Clone
            } else {
                CopyMethod::KernelCopy
            },
            "on a reflink filesystem this must report a clone, otherwise the kernel copy"
        );
        assert_eq!(std::fs::read(&dst).unwrap(), payload);
    }

    /// A refused clone must never leave the destination empty: the range copy
    /// fills the placeholder the ioctl opened.
    #[test]
    fn test_cascade_fills_the_clone_placeholder_on_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"hello world").unwrap();
        let inv = copy_file_cascade(&src, &dst, InstantCopyMode::Auto, |s, d| {
            std::fs::copy(s, d)
        })
        .expect("a copy");
        assert!(!inv.method.is_instant() || is_reflink_capable(&src));
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello world");
    }

    /// An empty file is a valid copy target on every path (the ioctl and the
    /// range copy both support a zero-length source).
    #[test]
    fn test_cascade_empty_file_copies() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("empty.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"").unwrap();
        let inv = copy_file_cascade(&src, &dst, InstantCopyMode::Auto, |s, d| {
            std::fs::copy(s, d)
        })
        .expect("a copy");
        assert_eq!(inv.bytes, 0);
        assert_eq!(std::fs::read(&dst).unwrap(), b"");
    }
}
