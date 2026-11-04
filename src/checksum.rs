/* =============================================================================
 * File          : checksum.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   On-demand file checksums for the File Explorer (HV-INT-01): SHA-256,
 *   SHA-512 and BLAKE3 over a single file, with a progress callback so a
 *   multi-gigabyte hash can be reported as a tracked job instead of blocking
 *   a request. Also compares two files by digest, and keeps the last few
 *   asynchronous job results so the client can read the hex after the job's
 *   progress event has gone.
 *
 * Dependencies:
 *   - blake3 (>= 1.8)
 *   - sha2 (>= 0.11)
 *
 * Usage:
 *   `hash_file_async` from the Explorer's `/api/explorer/checksum` handler;
 *   see docs/guides/explorer.md → "On-demand checksums".
 * =============================================================================
 */

//! On-demand checksums (SHA-256 / SHA-512 / BLAKE3) for the Explorer.
//!
//! DSM exposes a dedicated `SYNO.FileStation.MD5` module; this is the
//! HomeVault equivalent, with a modern digest instead of MD5. Three call
//! shapes are supported, and they all come from the same
//! [`hash_file_with_progress`] loop:
//!
//! 1. **verify one file** — `POST /api/explorer/checksum` on its own;
//! 2. **compare two files** — the optional `compare_with` path, so an
//!    administrator can prove a copy is identical instead of assuming it;
//! 3. **audit a tree** — the same endpoint called per file by a script (an
//!    uncached read of a 4 TB tree belongs in a job, and `async=true` gives
//!    one).
//!
//! Everything reads through a 64 KiB buffer and reports progress per block, so
//! the peak memory is one buffer per hash whatever the file size.

use std::io::Read;
use std::path::Path;
use std::time::Instant;

use blake3::Hasher as Blake3Hasher;
use sha2::{Digest, Sha256, Sha512};

/// Files up to this size are hashed inline (the request waits); larger ones
/// are pushed to a tracked job.
///
/// 8 MiB at a realistic 1 GB/s is under 10 ms — a file the user is looking at
/// can afford that. A 4 GB ISO cannot: it would hold an HTTP request (and a
/// Tokio worker's blocking-pool slot) for seconds, which is why anything
/// bigger becomes a job the tray can show.
pub const INLINE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Read size for the hashing loop. Matches the streaming copy's buffer.
const HASH_BLOCK_BYTES: usize = 64 * 1024;

/// Digest accepted by the checksum endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    Sha256,
    Sha512,
    /// BLAKE3: the same 256-bit output width as SHA-256 at several times the
    /// throughput, which is why it is offered for bulk verification.
    Blake3,
}

impl ChecksumAlgorithm {
    /// Parse a wire/query value. Case-insensitive and tolerant of the dashed
    /// spelling (`sha-256`), because both spellings are in the wild.
    ///
    /// `None` for anything unknown — the caller answers `400` rather than
    /// silently hashing with a digest the client did not ask for.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sha256" | "sha-256" => Some(Self::Sha256),
            "sha512" | "sha-512" => Some(Self::Sha512),
            "blake3" | "blake-3" => Some(Self::Blake3),
            _ => None,
        }
    }

    /// Stable wire slug.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Blake3 => "blake3",
        }
    }

    /// Hex digest length for this algorithm, used to sanity-check a
    /// client-supplied expected digest before a 4 GB hash is started.
    pub fn hex_len(self) -> usize {
        match self {
            Self::Sha256 | Self::Blake3 => 64,
            Self::Sha512 => 128,
        }
    }
}

impl Default for ChecksumAlgorithm {
    /// SHA-256: the digest the rest of the appliance already speaks
    /// (`X-Content-Sha256`, upload `checksums[]`, the share-link digest).
    fn default() -> Self {
        Self::Sha256
    }
}

/// What went wrong while hashing.
#[derive(Debug)]
pub enum ChecksumError {
    /// The path does not exist, is not a regular file, or could not be read.
    Io(std::io::Error),
    /// The path exists but is a directory or a device — nothing to hash.
    NotAFile,
    /// The caller's cancellation flag was set: the run stopped between two
    /// blocks, and no digest is reported.
    ///
    /// A tracked job must be *stoppable* — the tray's cancel button says so —
    /// and a hash is the one long transfer HomeVault can genuinely interrupt,
    /// because it owns the whole loop.
    Cancelled,
}

impl std::fmt::Display for ChecksumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::NotAFile => write!(f, "Not a regular file"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

impl std::error::Error for ChecksumError {}

impl From<std::io::Error> for ChecksumError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Result of one hash run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumOutcome {
    pub algorithm: ChecksumAlgorithm,
    /// Lowercase hex.
    pub checksum: String,
    pub file_size: u64,
    pub duration_ms: u64,
}

/// One of the three digest sinks, so the read loop exists once.
enum Sink {
    Sha256(Sha256),
    Sha512(Sha512),
    Blake3(Box<Blake3Hasher>),
}

impl Sink {
    fn new(algorithm: ChecksumAlgorithm) -> Self {
        match algorithm {
            ChecksumAlgorithm::Sha256 => Self::Sha256(Sha256::new()),
            ChecksumAlgorithm::Sha512 => Self::Sha512(Sha512::new()),
            ChecksumAlgorithm::Blake3 => Self::Blake3(Box::new(Blake3Hasher::new())),
        }
    }

    fn update(&mut self, block: &[u8]) {
        match self {
            Self::Sha256(h) => h.update(block),
            Self::Sha512(h) => h.update(block),
            Self::Blake3(h) => {
                h.update(block);
            }
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Sha256(h) => hex(&h.finalize()),
            Self::Sha512(h) => hex(&h.finalize()),
            Self::Blake3(h) => h.finalize().to_hex().to_string(),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // `write!` into the buffer: one allocation for the whole digest, where
        // `push_str(&format!(..))` allocated once per byte (64 times per file).
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Hash `path` with `algorithm`, calling `report(processed_bytes)` after every
/// block.
///
/// Blocking: callers run it on `spawn_blocking` (the hash is CPU- and
/// I/O-bound, and reading a stalled network mount must never park a Tokio
/// worker). `report` is called with the cumulative byte count, never a delta,
/// and returns `false` to **stop**: that is the cancellation path (the job's
/// hook flips a flag the callback reads), and it is checked between blocks so
/// a stop is neither delayed nor silently ignored.
pub fn hash_file_with_progress<F>(
    path: &Path,
    algorithm: ChecksumAlgorithm,
    mut report: F,
) -> Result<ChecksumOutcome, ChecksumError>
where
    F: FnMut(u64) -> bool,
{
    let started = Instant::now();
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(ChecksumError::NotAFile);
    }
    let mut reader = std::io::BufReader::with_capacity(HASH_BLOCK_BYTES, std::fs::File::open(path)?);
    let mut sink = Sink::new(algorithm);
    let mut block = vec![0u8; HASH_BLOCK_BYTES];
    let mut processed: u64 = 0;
    loop {
        let n = reader.read(&mut block)?;
        if n == 0 {
            break;
        }
        sink.update(&block[..n]);
        processed += n as u64;
        if !report(processed) {
            return Err(ChecksumError::Cancelled);
        }
    }
    Ok(ChecksumOutcome {
        algorithm,
        checksum: sink.finish(),
        file_size: meta.len(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Hash `path` without progress reporting (the inline path and the tests).
pub fn hash_file(path: &Path, algorithm: ChecksumAlgorithm) -> Result<ChecksumOutcome, ChecksumError> {
    hash_file_with_progress(path, algorithm, |_| true)
}

/// Hash `path` with a caller-supplied byte counter, on the blocking pool.
///
/// `progress` is an `Arc<AtomicU64>` the caller polls to emit job progress —
/// the same shape `run_tracked_copy` uses for directory copies, so a watcher
/// never has to hold a lock inside the hashing loop.
pub async fn hash_file_async(
    path: std::path::PathBuf,
    algorithm: ChecksumAlgorithm,
    progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<ChecksumOutcome, ChecksumError> {
    tokio::task::spawn_blocking(move || {
        hash_file_with_progress(&path, algorithm, |processed| {
            progress.store(processed, std::sync::atomic::Ordering::Relaxed);
            !cancel.load(std::sync::atomic::Ordering::Relaxed)
        })
    })
    .await
    .map_err(|e| ChecksumError::Io(std::io::Error::other(e.to_string())))?
}

/// `true` when two digests are equal, ignoring case and surrounding
/// whitespace (a checksum pasted from a download page usually has neither, but
/// a user can).
pub fn digests_match(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

/// The outcome of one asynchronous checksum job, kept so the client can read
/// the digest **after** the job's progress event has gone.
///
/// A progress event carries a message string and is dropped when the job
/// leaves the activity feed; a 4 GB BLAKE3 run that took two minutes must still
/// answer "what was the digest?" afterwards. Entries are bounded in number and
/// in age, so this can never become an unbounded cache of file metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutcome {
    pub job_id: String,
    /// Who asked for it. Stored here rather than read back from the progress
    /// manager: the job leaves that registry on a cleanup tick, and the
    /// authorisation of "may I read this digest?" must not expire with it.
    pub owner: Option<String>,
    pub state: ChecksumJobState,
    pub algorithm: ChecksumAlgorithm,
    /// The file that was hashed (as the caller asked for it).
    pub path: String,
    pub checksum: Option<String>,
    pub file_size: u64,
    pub duration_ms: u64,
    /// When `compare_with` was supplied: did the two digests match?
    pub matches: Option<bool>,
    pub error: Option<String>,
    /// Unix seconds, for the age cap.
    pub recorded_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumJobState {
    Complete,
    Failed,
}

impl ChecksumJobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

/// How many outcomes are remembered at once.
pub const MAX_REMEMBERED_OUTCOMES: usize = 256;
/// How long an outcome stays readable.
pub const OUTCOME_TTL_SECS: i64 = 15 * 60;

/// Bounded store of finished checksum jobs.
#[derive(Debug, Default)]
pub struct JobOutcomeStore {
    entries: std::collections::HashMap<String, JobOutcome>,
}

impl JobOutcomeStore {
    /// Insert one outcome, pruning expired entries first and evicting the
    /// oldest-looking ones when the cap is reached (a job id is a uuid, so
    /// "oldest" is by `recorded_at`, ties broken by id for determinism).
    pub fn insert(&mut self, outcome: JobOutcome, now: i64) {
        self.prune(now);
        if self.entries.len() >= MAX_REMEMBERED_OUTCOMES {
            if let Some(victim) = self
                .entries
                .values()
                .min_by_key(|o| (o.recorded_at, o.job_id.clone()))
                .map(|o| o.job_id.clone())
            {
                self.entries.remove(&victim);
            }
        }
        self.entries.insert(outcome.job_id.clone(), outcome);
    }

    /// Look one up, ignoring (and dropping) an expired entry.
    pub fn get(&mut self, job_id: &str, now: i64) -> Option<&JobOutcome> {
        let expired = self
            .entries
            .get(job_id)
            .map(|o| now.saturating_sub(o.recorded_at) >= OUTCOME_TTL_SECS)
            .unwrap_or(false);
        if expired {
            self.entries.remove(job_id);
            return None;
        }
        self.entries.get(job_id)
    }

    /// Drop everything older than [`OUTCOME_TTL_SECS`].
    pub fn prune(&mut self, now: i64) {
        self.entries
            .retain(|_, o| now.saturating_sub(o.recorded_at) < OUTCOME_TTL_SECS);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Process-wide outcome store (the async checksum jobs' results).
pub static JOB_OUTCOMES: std::sync::LazyLock<std::sync::Mutex<JobOutcomeStore>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(JobOutcomeStore::default()));

/// Record an outcome in the process-wide store.
pub fn remember_outcome(outcome: JobOutcome) {
    let now = outcome.recorded_at;
    let mut store = JOB_OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    store.insert(outcome, now);
}

/// Read back an outcome (pruning expired ones on the way).
pub fn remembered_outcome(job_id: &str) -> Option<JobOutcome> {
    let now = chrono::Utc::now().timestamp();
    let mut store = JOB_OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    store.get(job_id, now).cloned()
}

#[cfg(test)]
mod tests {
    //! Known-vector and edge-case tests — no DB, no HTTP.
    //! Run: `cargo test checksum::tests`
    use super::*;

    fn temp_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, bytes).expect("write");
        (dir, path)
    }

    #[test]
    fn test_parse_accepts_every_wire_spelling() {
        assert_eq!(
            ChecksumAlgorithm::parse("SHA256"),
            Some(ChecksumAlgorithm::Sha256)
        );
        assert_eq!(
            ChecksumAlgorithm::parse("sha-256"),
            Some(ChecksumAlgorithm::Sha256)
        );
        assert_eq!(
            ChecksumAlgorithm::parse("  sha512 "),
            Some(ChecksumAlgorithm::Sha512)
        );
        assert_eq!(
            ChecksumAlgorithm::parse("BLAKE3"),
            Some(ChecksumAlgorithm::Blake3)
        );
        assert_eq!(ChecksumAlgorithm::default(), ChecksumAlgorithm::Sha256);
    }

    #[test]
    fn test_parse_refuses_unknown_and_weak_md5() {
        // MD5 is what DSM offers; it is deliberately not accepted here.
        assert_eq!(ChecksumAlgorithm::parse("md5"), None);
        assert_eq!(ChecksumAlgorithm::parse("crc32"), None);
        assert_eq!(ChecksumAlgorithm::parse(""), None);
    }

    #[test]
    fn test_as_str_and_hex_len() {
        assert_eq!(ChecksumAlgorithm::Sha256.as_str(), "sha256");
        assert_eq!(ChecksumAlgorithm::Sha512.as_str(), "sha512");
        assert_eq!(ChecksumAlgorithm::Blake3.as_str(), "blake3");
        assert_eq!(ChecksumAlgorithm::Sha256.hex_len(), 64);
        assert_eq!(ChecksumAlgorithm::Sha512.hex_len(), 128);
        assert_eq!(ChecksumAlgorithm::Blake3.hex_len(), 64);
    }

    #[test]
    fn test_sha256_matches_known_vectors() {
        let (_d, path) = temp_file(b"");
        assert_eq!(
            hash_file(&path, ChecksumAlgorithm::Sha256).unwrap().checksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let (_d, path) = temp_file(b"abc");
        assert_eq!(
            hash_file(&path, ChecksumAlgorithm::Sha256).unwrap().checksum,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_sha512_matches_known_vectors() {
        let (_d, path) = temp_file(b"abc");
        let out = hash_file(&path, ChecksumAlgorithm::Sha512).unwrap();
        assert_eq!(
            out.checksum,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(out.checksum.len(), 128);
    }

    #[test]
    fn test_blake3_matches_known_vectors() {
        let (_d, path) = temp_file(b"abc");
        assert_eq!(
            hash_file(&path, ChecksumAlgorithm::Blake3).unwrap().checksum,
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        let (_d, path) = temp_file(b"");
        assert_eq!(
            hash_file(&path, ChecksumAlgorithm::Blake3).unwrap().checksum,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn test_progress_reports_a_cumulative_byte_count() {
        // 200 KiB crosses the 64 KiB block boundary three times: the loop must
        // report a *cumulative* count that only grows and ends on the file size.
        let (dir, _path) = temp_file(&[]);
        let path = dir.path().join("big.bin");
        std::fs::write(&path, vec![7u8; 200 * 1024]).unwrap();
        let mut seen: Vec<u64> = Vec::new();
        let out = hash_file_with_progress(&path, ChecksumAlgorithm::Sha256, |p| {
            seen.push(p);
            true
        })
        .expect("hash");
        assert_eq!(out.file_size, 200 * 1024);
        assert!(seen.len() >= 2, "progress must be reported per block: {seen:?}");
        assert!(
            seen.windows(2).all(|w| w[0] < w[1]),
            "cumulative, never a delta: {seen:?}"
        );
        assert_eq!(seen.last().copied(), Some(200 * 1024));
    }

    #[test]
    fn test_directory_is_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        match hash_file(dir.path(), ChecksumAlgorithm::Sha256) {
            Err(ChecksumError::NotAFile) => {}
            other => panic!("expected NotAFile, got {other:?}"),
        }
    }

    #[test]
    fn test_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        match hash_file(&dir.path().join("nope"), ChecksumAlgorithm::Sha256) {
            Err(ChecksumError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    fn outcome(job_id: &str, recorded_at: i64) -> JobOutcome {
        JobOutcome {
            job_id: job_id.to_string(),
            owner: Some("alice".into()),
            state: ChecksumJobState::Complete,
            algorithm: ChecksumAlgorithm::Sha256,
            path: "/data/file.bin".into(),
            checksum: Some("ab".repeat(32)),
            file_size: 4,
            duration_ms: 1,
            matches: None,
            error: None,
            recorded_at,
        }
    }

    #[test]
    fn test_outcome_store_round_trip_and_expiry() {
        let mut store = JobOutcomeStore::default();
        store.insert(outcome("job-1", 1_000), 1_000);
        assert_eq!(store.get("job-1", 1_000).map(|o| o.file_size), Some(4));
        // 15 minutes is the boundary: still readable just before, gone after.
        assert!(store.get("job-1", 1_000 + OUTCOME_TTL_SECS - 1).is_some());
        assert!(store.get("job-1", 1_000 + OUTCOME_TTL_SECS).is_none());
        assert!(store.is_empty(), "an expired entry is dropped, not just hidden");
    }

    #[test]
    fn test_outcome_store_evicts_the_oldest_at_the_cap() {
        let mut store = JobOutcomeStore::default();
        // One timestamp for the bulk: the cap test must not also exercise the
        // TTL, or the prune would remove most of the store first.
        for i in 0..MAX_REMEMBERED_OUTCOMES {
            store.insert(outcome(&format!("job-{i}"), 1_000), 1_000);
        }
        assert_eq!(store.len(), MAX_REMEMBERED_OUTCOMES);
        store.insert(outcome("job-new", 1_000), 1_000);
        assert_eq!(store.len(), MAX_REMEMBERED_OUTCOMES, "bounded");
        assert!(store.get("job-0", 1_000).is_none(), "oldest evicted");
        assert!(store.get("job-new", 1_000).is_some());
    }

    #[test]
    fn test_outcome_store_survives_a_clock_that_went_backwards() {
        let mut store = JobOutcomeStore::default();
        store.insert(outcome("job-1", 5_000), 5_000);
        assert!(
            store.get("job-1", 1_000).is_some(),
            "a clock correction must not expire a fresh result"
        );
    }

    #[test]
    fn test_checksum_job_state_slugs() {
        assert_eq!(ChecksumJobState::Complete.as_str(), "complete");
        assert_eq!(ChecksumJobState::Failed.as_str(), "failed");
    }

    #[test]
    fn test_a_false_return_stops_the_run_without_a_digest() {
        // The job's cancel hook flips a flag; the callback reports "stop" and
        // the run ends with no digest rather than an unverified one.
        let (dir, _path) = temp_file(&[]);
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, vec![3u8; 200 * 1024]).unwrap();
        let mut blocks = 0;
        let err = hash_file_with_progress(&path, ChecksumAlgorithm::Sha256, |_| {
            blocks += 1;
            false
        })
        .expect_err("must stop");
        assert!(matches!(err, ChecksumError::Cancelled), "{err:?}");
        assert_eq!(blocks, 1, "a stop is observed between blocks, immediately");
    }

    #[test]
    fn test_digests_match_ignores_case_and_padding() {
        assert!(digests_match(
            " BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD ",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        ));
        assert!(!digests_match("abc", "abd"));
        assert!(!digests_match("", "abd"));
    }

    #[tokio::test]
    async fn test_hash_file_async_publishes_the_byte_counter() {
        let (dir, _path) = temp_file(&[]);
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, vec![1u8; 100 * 1024]).unwrap();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let out = hash_file_async(path, ChecksumAlgorithm::Blake3, counter.clone(), cancel)
            .await
            .expect("hash");
        assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 100 * 1024);
        assert_eq!(out.checksum.len(), 64);
    }
}
