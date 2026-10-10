use axum::{
    body::Body,
    extract::{Multipart, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use crate::browser::{FileBrowser, TRASH_DIR_NAME};
use crate::handlers::AppState;
use crate::utils::ResponseHelper;

/// Disk identity triple `(model, transport, size)` keyed by bare kernel name.
type DiskIdentity = (Option<String>, Option<String>, Option<String>);

#[derive(Debug, Deserialize)]
pub struct ExplorerSearchQuery {
    pub path: String,
    pub query: String,
    pub max_results: Option<usize>,
}

/// Returns the canonical path a session may access, or `None` if the path is
/// outside the allowed roots. Admins bypass all checks and get the raw path
/// back; other sessions are restricted by `PathGuard`, whose returned path is
/// canonicalised (symlinks resolved, `..` normalised).
///
/// **Handlers must use the returned path for the actual filesystem work** —
/// never the raw input — otherwise the validate→use window is a TOCTOU gap
/// where a symlink swapped between the check and the open could smuggle the
/// operation outside the allowed roots.
fn validate_path_access(
    state: &Arc<AppState>,
    path: &str,
    session: &crate::storage::models::Session,
) -> Option<PathBuf> {
    if session.is_admin {
        Some(PathBuf::from(path))
    } else {
        state.path_guard.validate(path)
    }
}

/// Extracts a safe filename from a client-provided upload name.
/// Returns `None` if the name is empty, a dot component, or attempts to
/// escape the target directory (e.g. contains `..` or path separators).
fn sanitize_upload_filename(name: &str) -> Option<String> {
    let trimmed = name.trim();
    let file_name = std::path::Path::new(trimmed).file_name()?;
    let file_name = file_name.to_str()?;
    if file_name.is_empty() || file_name == "." || file_name == ".." {
        return None;
    }
    // Reject control characters that could corrupt the resulting path.
    if file_name.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(file_name.to_string())
}

/// Picks the trash directory for a given item path: the `.homevault-trash`
/// folder at the top of whichever allowed root contains it, so trash stays
/// visible/manageable within the same tree the user already has access to.
/// Falls back to a single app-owned trash dir (for admins browsing outside
/// any configured root) rather than scattering trash folders across
/// arbitrary system directories like `/etc` or `/root`.
fn resolve_trash_root(state: &Arc<AppState>, path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    for root in state.path_guard.allowed_roots() {
        if p.starts_with(root) {
            return root.join(TRASH_DIR_NAME);
        }
    }
    crate::config::paths::base_dir().join("trash")
}

/// Every trash directory a session can see: one per allowed root, plus the
/// app-owned fallback for admins (who can trash items outside any
/// configured root).
fn all_trash_roots(state: &Arc<AppState>, is_admin: bool) -> Vec<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = state
        .path_guard
        .allowed_roots()
        .iter()
        .map(|r| r.join(TRASH_DIR_NAME))
        .collect();
    if is_admin {
        roots.push(crate::config::paths::base_dir().join("trash"));
    }
    roots
}

// ---------------------------------------------------------------------------
// Shared HTTP file-streaming helpers (download + thumbnail + share links)
// ---------------------------------------------------------------------------

/// Files at or below this size get an `X-Content-Sha256` header on download
/// so the client can verify the transfer end-to-end. Larger files skip the
/// digest to avoid a full second read of a multi-gigabyte file just to hash
/// it before streaming.
pub(crate) const DIGEST_THRESHOLD: u64 = 64 * 1024 * 1024;

/// MIME type for a path's extension (mirrors the list used by the share-link
/// download handler).
pub(crate) fn content_type_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("txt") => "text/plain",
        Some("html") | Some("htm") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("mp4") => "video/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("xml") => "application/xml",
        Some("csv") => "text/csv",
        Some("md") => "text/markdown",
        _ => "application/octet-stream",
    }
}

/// Neutralises characters that could break a `Content-Disposition` header
/// (quotes, backslashes, control characters).
pub(crate) fn sanitize_download_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_control() || c == '"' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Parses a single HTTP `Range: bytes=a-b` header against `total` bytes.
///
/// Returns:
/// - `Ok(None)` — no `Range` header (serve the whole file, 200).
/// - `Ok(Some((start, end)))` — a satisfiable single range (206).
/// - `Err(())` — an absent/malformed/unsatisfiable range (→ 416).
pub(crate) fn parse_range(headers: &HeaderMap, total: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(raw) = headers.get("range").and_then(|h| h.to_str().ok()) else {
        return Ok(None);
    };
    let spec = raw.trim();
    if !spec.starts_with("bytes=") {
        return Err(());
    }
    let spec = &spec["bytes=".len()..];
    // Only a single range is supported; a multi-range request is rejected
    // rather than half-answered.
    if spec.contains(',') {
        return Err(());
    }
    if total == 0 {
        return Err(());
    }
    let Some(dash) = spec.find('-') else {
        return Err(());
    };
    let (start_s, end_s) = spec.split_at(dash);
    let end_s = &end_s[1..];

    if start_s.is_empty() {
        // Suffix form: last N bytes.
        let n: u64 = end_s.trim().parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        let start = total.saturating_sub(n);
        return Ok(Some((start, total - 1)));
    }

    let start: u64 = start_s.trim().parse().map_err(|_| ())?;
    if start >= total {
        return Err(());
    }
    let end = if end_s.trim().is_empty() {
        total - 1
    } else {
        end_s.trim().parse::<u64>().map_err(|_| ())?.min(total - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// SHA-256 (hex) of a file, read on a blocking thread. Used to attach an
/// integrity header to downloads of reasonably-sized files.
pub(crate) async fn file_sha256(path: &std::path::Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Option<String> {
        let mut hasher = Sha256::new();
        let f = std::fs::File::open(&p).ok()?;
        let mut reader = std::io::BufReader::new(f);
        let mut buf = [0u8; 1 << 16];
        loop {
            let n = reader.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Some(
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect(),
        )
    })
    .await
    .ok()
    .flatten()
}

/// Fills in the shared headers for a file response (content type, length,
/// disposition, ranges, digest) — used by the Explorer download and the
/// public share-link download so the two stay byte-for-byte consistent.
pub(crate) fn file_response_headers(
    status: StatusCode,
    content_type: &'static str,
    filename: &str,
    total: u64,
    range: Option<(u64, u64)>,
    digest: Option<String>,
) -> http::response::Builder {
    let length = match range {
        Some((start, end)) => end - start + 1,
        None => total,
    };
    let mut builder = http::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Content-Length", length.to_string())
        .header(
            "Content-Disposition",
            format!(
                "attachment; filename=\"{}\"",
                sanitize_download_filename(filename)
            ),
        )
        .header("Accept-Ranges", "bytes")
        .header("X-Content-Type-Options", "nosniff");
    if let Some((start, end)) = range {
        builder = builder.header(
            "Content-Range",
            format!("bytes {}-{}/{}", start, end, total),
        );
    }
    if let Some(d) = digest {
        builder = builder.header("X-Content-Sha256", d);
    }
    builder
}

/// Wraps a download body so the transfer-quota reservation made at stream
/// start is *refunded* if the client disconnects before the body is fully
/// consumed. The per-subject [`QuotaLockGuard`] is held for the whole stream,
/// so a concurrent transfer of the same subject can't race the reservation.
pub(crate) struct QuotaRefundStream<S> {
    inner: S,
    pool: Arc<sqlx::SqlitePool>,
    quotas: Vec<crate::transfer_quota::ApplicableQuota>,
    reserved: u64,
    upload: bool,
    completed: bool,
    _guard: crate::transfer_quota::QuotaLockGuard,
}

impl<S> Stream for QuotaRefundStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(item)) => Poll::Ready(Some(item)),
            Poll::Ready(None) => {
                self.completed = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for QuotaRefundStream<S> {
    fn drop(&mut self) {
        // Only refund a stream that never reached its end (client disconnect,
        // error) — a fully-consumed download keeps its reservation.
        if !self.completed && self.reserved > 0 {
            let pool = self.pool.clone();
            let quotas = self.quotas.clone();
            let reserved = self.reserved;
            let upload = self.upload;
            tokio::spawn(async move {
                crate::transfer_quota::refund_usage(pool, &quotas, reserved, upload).await;
            });
        }
    }
}

/// Starts a background job that copies a directory tree while reporting
/// progress over the existing SSE progress system (`src/sse/progress.rs`),
/// the same `job_id`/`tokio::spawn`/`ProgressEmitter` pattern already used
/// by `src/handlers/rescue.rs`'s ddrescue clone job. Returns immediately.
///
/// The job must be registered via `PROGRESS_MANAGER.create_job` (not just a
/// self-generated UUID) — `ProgressEmitter::progress`/`complete`/`fail` are
/// all no-ops for a job_id that was never `create_job`'d, since they look it
/// up in the manager's map before doing anything.
fn spawn_tracked_copy_job(source: String, destination: String, owner: String) -> (String, String) {
    // `owner` scopes the job in the desktop activity feed — a non-admin only
    // sees their own copy/move jobs, never another user's file paths.
    let job_id = crate::sse::PROGRESS_MANAGER.create_job_with_owner(
        crate::sse::JobType::Custom("explorer_copy".to_string()),
        "Starting copy...",
        Some(owner),
    );
    let jid = job_id.clone();
    tokio::spawn(async move {
        run_tracked_copy(&jid, &source, &destination, false).await;
    });
    (job_id.clone(), format!("/api/progress/{}", job_id))
}

/// Same as `spawn_tracked_copy_job`, but deletes the source after a
/// successful copy — used for the cross-filesystem move fallback.
fn spawn_tracked_move_job(source: String, destination: String, owner: String) -> (String, String) {
    let job_id = crate::sse::PROGRESS_MANAGER.create_job_with_owner(
        crate::sse::JobType::Custom("explorer_move".to_string()),
        "Starting move...",
        Some(owner),
    );
    let jid = job_id.clone();
    tokio::spawn(async move {
        run_tracked_copy(&jid, &source, &destination, true).await;
    });
    (job_id.clone(), format!("/api/progress/{}", job_id))
}

async fn run_tracked_copy(
    job_id: &str,
    source: &str,
    destination: &str,
    delete_source_after: bool,
) {
    use crate::sse::ProgressEmitter;

    let count_path = std::path::PathBuf::from(source);
    let total = tokio::task::spawn_blocking(move || FileBrowser::count_files(&count_path))
        .await
        .unwrap_or(0);

    let copied = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Poll the shared counter every 300ms and emit progress until the copy
    // finishes (the copy task aborts this loop itself when done).
    let copied_poll = copied.clone();
    let jid_poll = job_id.to_string();
    let poll_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let c = copied_poll.load(std::sync::atomic::Ordering::Relaxed);
            let pct = if total > 0 {
                ((c as f64 / total as f64) * 100.0).min(99.0)
            } else {
                50.0
            };
            ProgressEmitter::progress(&jid_poll, pct, &format!("{}/{} files", c, total));
        }
    });

    let src_path = std::path::PathBuf::from(source);
    let dst_path = std::path::PathBuf::from(destination);
    let copied_for_copy = copied.clone();
    let copy_result = tokio::task::spawn_blocking({
        let mut created: Vec<std::path::PathBuf> = Vec::new();
        move || {
            let result = FileBrowser::copy_dir_all_tracked(
                &src_path,
                &dst_path,
                &copied_for_copy,
                &mut created,
            );
            (result, created)
        }
    })
    .await;

    poll_handle.abort();

    let (copy_result, mut created): (
        Result<(), crate::browser::BrowserError>,
        Vec<std::path::PathBuf>,
    ) = match copy_result {
        Ok(pair) => pair,
        Err(e) => (
            Err(crate::browser::BrowserError::Io(std::io::Error::other(
                e.to_string(),
            ))),
            Vec::new(),
        ),
    };

    let source_owned = source.to_string();
    let final_result: Result<(), String> = match copy_result {
        Ok(()) if delete_source_after => {
            tokio::task::spawn_blocking(move || FileBrowser::delete_item(&source_owned, true))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
        }
        Ok(()) => Ok(()),
        Err(e) => Err(e.to_string()),
    };

    if let Err(ref e) = final_result {
        // The copy failed partway: remove exactly the paths this run created
        // (children before parents, deepest-first) so the destination isn't
        // left with a misleading half-populated tree. Anything that existed
        // before the copy started is left untouched.
        cleanup_created_paths(&mut created);
        tracing::warn!(
            "Tracked copy/move failed; cleaned {} partial destination path(s): {}",
            created.len(),
            e
        );
    }

    match final_result {
        Ok(()) => {
            ProgressEmitter::progress(job_id, 100.0, "Done");
            ProgressEmitter::complete(job_id, "Done");
        }
        Err(e) => {
            ProgressEmitter::fail(job_id, &e);
        }
    }
}

/// Removes the paths a failed tracked copy created, deepest-first so parents
/// are only removed once their children are gone. Only *empty* directories
/// are removed (`remove_dir`, not `remove_dir_all`) — if anything unexpected
/// is still in one, it is left rather than recursively nuked.
fn cleanup_created_paths(created: &mut [std::path::PathBuf]) {
    for p in created.iter().rev() {
        let _ = if p.is_dir() {
            std::fs::remove_dir(p)
        } else {
            std::fs::remove_file(p)
        };
    }
}

#[derive(Debug, Deserialize)]
pub struct ExplorerListQuery {
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerMkdirQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerDeleteQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerDownloadQuery {
    pub path: String,
    /// Optional short-lived, single-use download ticket. When present it
    /// authorises the stream without a session header — and *never* carries
    /// the session token itself (that would leak into access logs). Minted by
    /// `POST /api/explorer/download-ticket`.
    pub ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerUploadQuery {
    pub path: String,
    /// `overwrite` (default) | `keep_both` | `skip` — how to handle a file
    /// whose name already exists in the target directory.
    pub strategy: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerRenameQuery {
    pub path: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerCopyQuery {
    pub source: String,
    pub destination: String,
    /// `overwrite` (default) | `keep_both` | `skip`.
    pub strategy: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerMoveQuery {
    pub source: String,
    pub destination: String,
    /// `overwrite` (default) | `keep_both` | `skip`.
    pub strategy: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerPreviewQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerPermissionsQuery {
    pub path: String,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerExtractQuery {
    pub path: String,
    pub destination: String,
}

#[derive(Debug, Deserialize)]
pub struct ExplorerThumbnailQuery {
    pub path: String,
    /// Optional multi-use, directory-scoped thumbnail ticket (see
    /// `crate::media_ticket`). Never the session token.
    pub ticket: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub path: String,
    pub mount_point: String,
    pub filesystem: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub category: String,
    /// Filesystem label, if the user gave one (e.g. a USB stick named
    /// "Backup"). `None` for most local partitions and all network mounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Physical-disk identity (from the parent block device), used by the
    /// folder picker when there is no label to show. `None` for network /
    /// rclone mounts, which have no backing block device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

/// Per-device label + parent-disk identity, harvested from one `lsblk` call.
#[derive(Default, Clone)]
struct BlockDeviceMeta {
    label: Option<String>,
    vendor: Option<String>,
    model: Option<String>,
    serial: Option<String>,
}

/// `KEY="value" KEY="value"` (lsblk `-P`) → map. Mirrors
/// `DiskService::parse_lsblk_pairs` (kept local to avoid widening that
/// frozen type's surface).
fn parse_lsblk_pairs(line: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut rest = line.trim();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        rest = &rest[eq + 1..];
        if !rest.starts_with('"') {
            break;
        }
        rest = &rest[1..];
        let Some(end) = rest.find('"') else { break };
        map.insert(key, rest[..end].to_string());
        rest = rest[end + 1..].trim_start();
    }
    map
}

/// One `lsblk` pass → `{ /dev/path : BlockDeviceMeta }`. Each partition row
/// gets its own LABEL plus the MODEL/SERIAL/VENDOR of its parent disk (via
/// `PKNAME`, falling back to `strip_partition_suffix`). Best-effort: a
/// missing/failed `lsblk` just yields an empty map.
fn collect_block_device_meta() -> std::collections::HashMap<String, BlockDeviceMeta> {
    use std::collections::HashMap;
    let mut out: HashMap<String, BlockDeviceMeta> = HashMap::new();

    let clean = |v: Option<&String>| -> Option<String> {
        let s = v.map(|s| s.trim())?;
        if s.is_empty() || s.eq_ignore_ascii_case("unknown") {
            None
        } else {
            Some(s.to_string())
        }
    };

    let output = match std::process::Command::new("lsblk")
        .args([
            "-P",
            "-o",
            "PATH,NAME,PKNAME,TYPE,LABEL,MODEL,SERIAL,VENDOR",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return out,
    };
    let text = String::from_utf8_lossy(&output.stdout);

    // First pass: disk identity keyed by bare NAME (e.g. "nvme0n1", "sda").
    let mut disk_ident: HashMap<String, DiskIdentity> = HashMap::new();
    let rows: Vec<HashMap<String, String>> = text
        .lines()
        .map(parse_lsblk_pairs)
        .filter(|f| !f.is_empty())
        .collect();
    for f in &rows {
        if f.get("TYPE").map(|t| t == "disk").unwrap_or(false) {
            if let Some(name) = f.get("NAME") {
                disk_ident.insert(
                    name.clone(),
                    (
                        clean(f.get("VENDOR")),
                        clean(f.get("MODEL")),
                        clean(f.get("SERIAL")),
                    ),
                );
            }
        }
    }

    // Second pass: attach label + parent identity to every path.
    for f in &rows {
        let Some(path) = f.get("PATH").filter(|s| !s.is_empty()).cloned() else {
            continue;
        };
        let name = f.get("NAME").cloned().unwrap_or_default();
        let parent = f
            .get("PKNAME")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| strip_partition_suffix(&name));
        let ident = disk_ident.get(&parent).or_else(|| disk_ident.get(&name));
        out.insert(
            path,
            BlockDeviceMeta {
                label: clean(f.get("LABEL")),
                vendor: ident.and_then(|i| i.0.clone()),
                model: ident.and_then(|i| i.1.clone()),
                serial: ident.and_then(|i| i.2.clone()),
            },
        );
    }
    out
}

#[derive(Debug, Serialize)]
pub struct DisksResponse {
    pub system: Vec<DiskInfo>,
    pub internal: Vec<DiskInfo>,
    pub raid: Vec<DiskInfo>,
    pub external: Vec<DiskInfo>,
    pub cloud: Vec<DiskInfo>,
    pub network: Vec<DiskInfo>,
}

/// Lists directory contents at the given path.
#[axum_macros::debug_handler]
pub async fn handle_explorer_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerListQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let path = params.path.unwrap_or_else(|| "/".to_string());

    let safe_path = match state.auth.get_session(session_token).await {
        Ok(session) => match validate_path_access(&state, &path, &session) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Explorer list blocked: path '{}' outside allowed roots",
                    path
                );
                return ResponseHelper::json_error(
                    "Access denied: path outside allowed directory".to_string(),
                );
            }
        },
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    match FileBrowser::list_directory(&safe_path.to_string_lossy()).await {
        Ok(listing) => ResponseHelper::json_success(listing),
        Err(e) => ResponseHelper::json_error(format!("Failed to list directory: {}", e)),
    }
}

/// Recursively searches for files matching a query within a path.
#[axum_macros::debug_handler]
pub async fn handle_explorer_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerSearchQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let path = if params.path.starts_with('/') {
        params.path.clone()
    } else {
        format!("/{}", params.path)
    };

    let safe_path = match state.auth.get_session(session_token).await {
        Ok(session) => match validate_path_access(&state, &path, &session) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Explorer search blocked: path '{}' outside allowed roots",
                    path
                );
                return ResponseHelper::json_error(
                    "Access denied: path outside allowed directory".to_string(),
                );
            }
        },
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let max_results = params.max_results.unwrap_or(100);

    match FileBrowser::search(&safe_path.to_string_lossy(), &params.query, max_results).await {
        Ok(results) => ResponseHelper::json_success(serde_json::json!({
            "query": params.query,
            "path": safe_path.to_string_lossy(),
            "count": results.len(),
            "results": results
        })),
        Err(e) => ResponseHelper::json_error(format!("Search failed: {}", e)),
    }
}

/// Creates a new directory at the specified path.
#[axum_macros::debug_handler]
pub async fn handle_explorer_mkdir(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerMkdirQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            let validated = match validate_path_access(&state, &params.path, &session) {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "Explorer mkdir blocked: path '{}' outside allowed roots",
                        params.path
                    );
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            };
            let path = validated.to_string_lossy().into_owned();
            match tokio::task::spawn_blocking(move || FileBrowser::create_directory(&path)).await {
                Ok(Ok(_)) => ResponseHelper::json_success(
                    serde_json::json!({"message": "Directory created successfully"}),
                ),
                Ok(Err(e)) => {
                    ResponseHelper::json_error(format!("Failed to create directory: {}", e))
                }
                Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
            }
        }
        Err(_) => ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }
}

/// Permanently deletes a file or directory (recursive).
///
/// **Admin-only.** This is the irreversible bypass of the trash system; the
/// Explorer UI never calls it (the Delete button trashes instead), and the
/// purge/empty-trash flows call `FileBrowser::delete_item` server-side. It is
/// kept only as an admin escape hatch.
#[axum_macros::debug_handler]
pub async fn handle_explorer_delete(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerDeleteQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) if session.is_admin => {}
        Ok(_) => return ResponseHelper::json_forbidden("Admin required".to_string()),
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let path = params.path.clone();
    match tokio::task::spawn_blocking(move || FileBrowser::delete_item(&path, true)).await {
        Ok(Ok(_)) => ResponseHelper::json_success(
            serde_json::json!({"message": "Item deleted successfully"}),
        ),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to delete item: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Streams a file to the client with MIME-type detection, HTTP range support
/// (resumable downloads / video seeking) and an optional integrity digest.
///
/// Authorisation is either a short-lived single-use **download ticket**
/// (`?ticket=…`, minted by `POST /api/explorer/download-ticket` after the
/// path and quota were checked) or the usual `x-session-token` header. The
/// session token is never accepted in the query string.
#[axum_macros::debug_handler]
pub async fn handle_explorer_download(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerDownloadQuery>,
    headers: HeaderMap,
) -> Response {
    // 1. Resolve authorisation → (username, canonical file path).
    let (username, file_path) = if let Some(ticket) = params.ticket.as_deref() {
        match crate::media_ticket::consume_download_ticket(ticket, &params.path) {
            Some((username, path)) => (username, PathBuf::from(path)),
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    "Invalid or expired download ticket".to_string(),
                )
                    .into_response();
            }
        }
    } else {
        let session_token = headers
            .get("x-session-token")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if session_token.is_empty() {
            return (
                StatusCode::UNAUTHORIZED,
                "No session token provided".to_string(),
            )
                .into_response();
        }
        let session = match state.auth.get_session(session_token).await {
            Ok(s) => s,
            Err(_) => {
                return (StatusCode::UNAUTHORIZED, "Session invalid".to_string()).into_response();
            }
        };
        let Some(validated) = validate_path_access(&state, &params.path, &session) else {
            tracing::warn!(
                "Explorer download blocked: path '{}' outside allowed roots",
                params.path
            );
            return (
                StatusCode::FORBIDDEN,
                "Access denied: path outside allowed directory".to_string(),
            )
                .into_response();
        };
        (session.username, validated)
    };

    if !file_path.exists() {
        return (StatusCode::NOT_FOUND, "File not found".to_string()).into_response();
    }
    if file_path.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            "Cannot download directory".to_string(),
        )
            .into_response();
    }

    // 2. Open + stat.
    let mut file = match tokio::fs::File::open(&file_path).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open file: {}", e),
            )
                .into_response();
        }
    };
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get file metadata: {}", e),
            )
                .into_response();
        }
    };
    let total = metadata.len();

    // 3. Range negotiation.
    let range = match parse_range(&headers, total) {
        Ok(r) => r,
        Err(()) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header("Content-Range", format!("bytes */{}", total))
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::RANGE_NOT_SATISFIABLE.into_response());
        }
    };
    let status = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let send_len = match range {
        Some((start, end)) => end - start + 1,
        None => total,
    };

    // 4. Transfer quota: load, lock the subject(s), check, then reserve. The
    //    lock is held by the response stream so a concurrent transfer can't
    //    race the reservation; the reservation is refunded if the client
    //    disconnects before the body is fully consumed.
    let download_quotas =
        match crate::transfer_quota::applicable_download_quotas(state.db.pool(), &username).await {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!("Failed to load download quota for '{}': {}", username, e);
                Vec::new()
            }
        };
    let guard = crate::transfer_quota::acquire_quota_locks(&download_quotas).await;
    if let Err(exceeded) = crate::transfer_quota::check_quota(&download_quotas, send_len) {
        return (StatusCode::TOO_MANY_REQUESTS, exceeded.to_string()).into_response();
    }
    crate::transfer_quota::record_usage(state.db.pool(), &download_quotas, send_len, false).await;

    // 5. Optional integrity digest (small files only — avoids a second full
    //    read of a huge file just to hash it).
    let digest = if total <= DIGEST_THRESHOLD {
        file_sha256(&file_path).await
    } else {
        None
    };

    // 6. Build the (range-limited) body stream.
    let content_type = content_type_for_path(&file_path);
    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    let body = if total == 0 {
        Body::empty()
    } else {
        let start = range.map(|(s, _)| s).unwrap_or(0);
        if start > 0 {
            if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to seek file: {}", e),
                )
                    .into_response();
            }
        }
        let limited = file.take(send_len);
        let stream = tokio_util::io::ReaderStream::new(limited);
        Body::from_stream(QuotaRefundStream {
            inner: stream,
            pool: state.db.pool(),
            quotas: download_quotas,
            reserved: send_len,
            upload: false,
            completed: false,
            _guard: guard,
        })
    };

    file_response_headers(status, content_type, filename, total, range, digest)
        .body(body)
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create streaming response",
            )
                .into_response()
        })
}

/// `POST /api/explorer/download-ticket` — validates the path and the caller's
/// download quota, then mints a short-lived single-use ticket the browser can
/// hand to `GET /api/explorer/download?ticket=…`. Doing the quota check here
/// (rather than only in the stream) lets the frontend surface a "quota
/// exceeded" message instead of a silently-failed download.
#[axum_macros::debug_handler]
pub async fn handle_explorer_download_ticket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ExplorerPreviewQuery>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }
    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let Some(path) = validate_path_access(&state, &params.path, &session) else {
        tracing::warn!(
            "Download ticket blocked: path '{}' outside allowed roots",
            params.path
        );
        return ResponseHelper::json_forbidden(
            "Access denied: path outside allowed directory".to_string(),
        );
    };
    if !path.is_file() {
        return ResponseHelper::json_error("File not found".to_string());
    }

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let quotas =
        crate::transfer_quota::applicable_download_quotas(state.db.pool(), &session.username)
            .await
            .unwrap_or_default();
    let _guard = crate::transfer_quota::acquire_quota_locks(&quotas).await;
    if let Err(exceeded) = crate::transfer_quota::check_quota(&quotas, size) {
        return (StatusCode::TOO_MANY_REQUESTS, exceeded.to_string()).into_response();
    }

    let canonical = std::fs::canonicalize(&path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    let ticket = crate::media_ticket::issue_download_ticket(&session.username, &canonical);
    ResponseHelper::json_success(serde_json::json!({
        "ticket": ticket,
        "url": format!("/api/explorer/download?ticket={}", ticket),
    }))
}

/// Accepts multipart file uploads and writes them to the specified path.
#[axum_macros::debug_handler]
pub async fn handle_explorer_upload(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerUploadQuery>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let upload_path = match validate_path_access(&state, &params.path, &session) {
        Some(p) => p,
        None => {
            tracing::warn!(
                "Explorer upload blocked: path '{}' outside allowed roots",
                params.path
            );
            return ResponseHelper::json_error(
                "Access denied: path outside allowed directory".to_string(),
            );
        }
    };

    // Enforce any upload transfer quota configured on the Users/Groups
    // pages (see src/transfer_quota.rs). Checked once up front against
    // already-recorded usage, then re-checked after every chunk against
    // usage-so-far-in-this-request so a single huge upload can't blow past
    // the limit before the first check would otherwise fire again. The
    // per-subject lock is held for the whole request so two concurrent
    // uploads of the same subject can't both pass a check on stale usage.
    let upload_quotas =
        match crate::transfer_quota::applicable_upload_quotas(state.db.pool(), &session.username)
            .await
        {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!(
                    "Failed to load upload quota for '{}': {}",
                    session.username,
                    e
                );
                Vec::new()
            }
        };
    let _quota_guard = crate::transfer_quota::acquire_quota_locks(&upload_quotas).await;
    if let Err(exceeded) = crate::transfer_quota::check_quota(&upload_quotas, 0) {
        return ResponseHelper::json_error(exceeded.to_string());
    }
    let mut session_upload_bytes: u64 = 0;

    if !upload_path.exists() {
        return ResponseHelper::json_error(format!("Upload path does not exist: {}", params.path));
    }

    if !upload_path.is_dir() {
        return ResponseHelper::json_error("Upload path must be a directory".to_string());
    }

    // Optional client-supplied SHA-256 to verify against. Only meaningful for
    // a single-file upload; when present the stored file must hash to exactly
    // this value or the upload is rejected and the partial file removed.
    let expected_sha256 = headers
        .get("x-expected-sha256")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    let mut uploaded_files = Vec::new();
    let mut uploaded_checksums = Vec::new();
    let mut skipped_files: Vec<String> = Vec::new();
    let collision_strategy = crate::browser::CollisionStrategy::parse(params.strategy.as_deref());

    while let Some(mut field) = match multipart.next_field().await {
        Ok(f) => f,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to read multipart field: {}", e));
        }
    } {
        // Never trust the client-provided filename: extract only the final
        // path component and reject anything that could traverse directories.
        let raw_name = field.file_name().unwrap_or("unknown").to_string();
        let mut filename = match sanitize_upload_filename(&raw_name) {
            Some(name) => name,
            None => {
                return ResponseHelper::json_error(format!(
                    "Rejected upload with unsafe filename: {}",
                    raw_name
                ));
            }
        };
        let mut file_path = upload_path.join(&filename);

        // Defense in depth: ensure the resolved destination stays inside the
        // validated upload directory (also guards against symlink tricks).
        if !file_path.starts_with(&upload_path) {
            return ResponseHelper::json_error(format!(
                "Rejected upload with unsafe filename: {}",
                filename
            ));
        }

        // Apply the caller's collision strategy for a name that already
        // exists in the target directory.
        match collision_strategy {
            crate::browser::CollisionStrategy::Skip if file_path.exists() => {
                // Drain this field's body so the multipart parser can advance.
                while let Ok(Some(_)) = field.chunk().await {}
                skipped_files.push(filename);
                continue;
            }
            crate::browser::CollisionStrategy::KeepBoth => {
                file_path = crate::browser::non_colliding_path(&file_path);
                if let Some(n) = file_path.file_name().and_then(|n| n.to_str()) {
                    filename = n.to_string();
                }
            }
            _ => {}
        }

        // Stream into a sibling temp file first, not straight into
        // `file_path`. Writing directly into `file_path` would truncate any
        // existing file of the same name immediately, before a single byte
        // is verified — a dropped connection, crash, or power loss mid-
        // upload would then leave a corrupt, truncated file where a good one
        // used to be, with the original content unrecoverable. The temp file
        // is only ever moved into place with a single atomic `rename`, after
        // its checksum has been confirmed.
        let tmp_name = format!(".{}.hvtmp-{}", filename, uuid::Uuid::new_v4());
        let tmp_path = upload_path.join(&tmp_name);

        let mut file = match tokio::fs::File::create(&tmp_path).await {
            Ok(f) => f,
            Err(e) => {
                return ResponseHelper::json_error(format!(
                    "Failed to create file {}: {}",
                    filename, e
                ));
            }
        };

        // Write in chunks to avoid loading the entire file in memory, hashing
        // the bytes on the fly so we can verify the stored file end-to-end.
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let mut total_bytes: u64 = 0;
        loop {
            match field.chunk().await {
                Ok(Some(bytes)) => {
                    let len = bytes.len();
                    match file.write_all(&bytes).await {
                        Ok(_) => {
                            hasher.update(&bytes);
                            total_bytes += len as u64;

                            if let Err(exceeded) = crate::transfer_quota::check_quota(
                                &upload_quotas,
                                session_upload_bytes + total_bytes,
                            ) {
                                drop(file);
                                let _ = tokio::fs::remove_file(&tmp_path).await;
                                return ResponseHelper::json_error(exceeded.to_string());
                            }
                        }
                        Err(e) => {
                            let _ = tokio::fs::remove_file(&tmp_path).await;
                            return ResponseHelper::json_error(format!(
                                "Failed to write file {}: {}",
                                filename, e
                            ));
                        }
                    }
                }
                Ok(None) => break, // EOF
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return ResponseHelper::json_error(format!(
                        "Failed to read upload data for {}: {}",
                        filename, e
                    ));
                }
            }
        }

        // Flush and fsync so the bytes are durable on disk before we trust
        // this copy or rename it into place.
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return ResponseHelper::json_error(format!("Failed to flush file {}: {}", filename, e));
        }
        if let Err(e) = file.sync_all().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return ResponseHelper::json_error(format!("Failed to sync file {}: {}", filename, e));
        }
        drop(file);

        let checksum = hasher
            .finalize()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        // If the client sent an expected checksum, enforce it (single-file
        // uploads only — a multi-file upload can't map one header to many).
        if let Some(expected) = &expected_sha256 {
            if expected != &checksum {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return ResponseHelper::json_error(format!(
                    "Integrity check failed for {}: expected SHA-256 {}, got {}",
                    filename, expected, checksum
                ));
            }
        }

        // Only now — verified durable and (optionally) checksum-matched —
        // atomically swap the temp file into its final name, and make the
        // rename itself durable with a parent-dir fsync.
        if let Err(e) = tokio::fs::rename(&tmp_path, &file_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return ResponseHelper::json_error(format!(
                "Failed to finalize upload for {}: {}",
                filename, e
            ));
        }
        FileBrowser::fsync_parent_dir(&file_path);

        // Only count bytes that actually landed on disk under their final
        // name — an aborted/failed field above never reaches here.
        session_upload_bytes += total_bytes;

        tracing::debug!(
            "Uploaded file {} ({} bytes, sha256={}) to {}",
            filename,
            total_bytes,
            checksum,
            file_path.display()
        );
        uploaded_files.push(filename);
        uploaded_checksums.push(checksum);
    }

    crate::transfer_quota::record_usage(
        state.db.pool(),
        &upload_quotas,
        session_upload_bytes,
        true,
    )
    .await;

    ResponseHelper::json_success(serde_json::json!({
        "message": format!("Uploaded {} file(s)", uploaded_files.len()),
        "files": uploaded_files,
        "checksums": uploaded_checksums,
        "skipped": skipped_files
    }))
}

// ---------------------------------------------------------------------------
// Resumable chunked uploads
// ---------------------------------------------------------------------------
//
// The multipart `/explorer/upload` endpoint above is fine for small files but
// an interrupted large upload has to restart from zero. This session-based
// flow lets the client stream a file in chunks (`POST /upload/chunk`), retry
// a failed chunk from the server's authoritative offset, and only finalise it
// (`POST /upload/complete`) once every byte is durably on disk. Aborting
// (`DELETE /upload/session`) removes the partial temp file.

use parking_lot::Mutex as ParkingMutex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const UPLOAD_SESSION_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
struct UploadSession {
    dir: String,
    filename: String,
    tmp_path: PathBuf,
    /// Bytes durably written so far — the client resumes from here.
    offset: u64,
    total_bytes: u64,
    username: String,
    strategy: crate::browser::CollisionStrategy,
    expected_sha256: Option<String>,
    created_at: Instant,
}

static UPLOAD_SESSIONS: LazyLock<ParkingMutex<HashMap<String, UploadSession>>> =
    LazyLock::new(|| ParkingMutex::new(HashMap::new()));

/// Removes expired sessions and their leftover temp files. Called opportunistically
/// on every session-touching endpoint so stale state never accumulates.
fn prune_upload_sessions() {
    let now = Instant::now();
    let mut map = UPLOAD_SESSIONS.lock();
    let expired: Vec<String> = map
        .iter()
        .filter(|(_, s)| now.duration_since(s.created_at) >= UPLOAD_SESSION_TTL)
        .map(|(k, _)| k.clone())
        .collect();
    for key in expired {
        if let Some(s) = map.remove(&key) {
            let _ = std::fs::remove_file(&s.tmp_path);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UploadSessionQuery {
    pub path: String,
    pub filename: String,
    /// Expected total file size in bytes (used for quota enforcement).
    pub size: u64,
    pub strategy: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadChunkQuery {
    pub session_id: String,
    pub offset: u64,
}

#[derive(Debug, Deserialize)]
pub struct UploadSessionIdQuery {
    pub session_id: String,
}

/// `POST /api/explorer/upload/session` — open a resumable upload slot for one
/// file. Validates the destination directory, the filename and the quota
/// up-front, and creates the hidden temp file the chunks will be written into.
#[axum_macros::debug_handler]
pub async fn handle_explorer_upload_session_start(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UploadSessionQuery>,
    headers: HeaderMap,
) -> Response {
    prune_upload_sessions();

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }
    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let dir = match validate_path_access(&state, &params.path, &session) {
        Some(p) => p,
        None => {
            tracing::warn!(
                "Upload session blocked: path '{}' outside allowed roots",
                params.path
            );
            return ResponseHelper::json_error(
                "Access denied: path outside allowed directory".to_string(),
            );
        }
    };
    if !dir.is_dir() {
        return ResponseHelper::json_error("Upload path must be a directory".to_string());
    }
    let Some(filename) = sanitize_upload_filename(&params.filename) else {
        return ResponseHelper::json_error("Rejected upload with unsafe filename".to_string());
    };
    let strategy = crate::browser::CollisionStrategy::parse(params.strategy.as_deref());
    let expected_sha256 = params
        .sha256
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    // Quota: reject an over-limit file before creating anything.
    let upload_quotas =
        match crate::transfer_quota::applicable_upload_quotas(state.db.pool(), &session.username)
            .await
        {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!(
                    "Failed to load upload quota for '{}': {}",
                    session.username,
                    e
                );
                Vec::new()
            }
        };
    let _guard = crate::transfer_quota::acquire_quota_locks(&upload_quotas).await;
    if let Err(exceeded) = crate::transfer_quota::check_quota(&upload_quotas, params.size) {
        return (StatusCode::TOO_MANY_REQUESTS, exceeded.to_string()).into_response();
    }

    let tmp_path = dir.join(format!(".{}.hvupl-{}", filename, uuid::Uuid::new_v4()));
    if let Err(e) = tokio::fs::File::create(&tmp_path).await {
        return ResponseHelper::json_error(format!("Failed to create upload slot: {}", e));
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    UPLOAD_SESSIONS.lock().insert(
        session_id.clone(),
        UploadSession {
            dir: dir.to_string_lossy().into_owned(),
            filename,
            tmp_path,
            offset: 0,
            total_bytes: params.size,
            username: session.username.clone(),
            strategy,
            expected_sha256,
            created_at: Instant::now(),
        },
    );

    ResponseHelper::json_success(serde_json::json!({ "session_id": session_id }))
}

/// `POST /api/explorer/upload/chunk?session_id=…&offset=…` — write one chunk
/// at the server's authoritative offset. A chunk arriving out of order is
/// rejected with a 409 carrying the current offset, so the client knows where
/// to resume after a partial write.
#[axum_macros::debug_handler]
pub async fn handle_explorer_upload_chunk(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UploadChunkQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    prune_upload_sessions();

    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    let session = {
        let session_map = UPLOAD_SESSIONS.lock();
        let Some(s) = session_map.get(&params.session_id).cloned() else {
            return ResponseHelper::json_error("Upload session not found or expired".to_string());
        };
        s
    };

    if params.offset != session.offset {
        return (
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({
                "success": false,
                "offset": session.offset,
                "message": format!("Offset mismatch: expected {}, got {}", session.offset, params.offset)
            })),
        )
            .into_response();
    }
    if body.is_empty() && session.total_bytes > 0 {
        return ResponseHelper::json_error("Empty chunk".to_string());
    }
    let next = params.offset + body.len() as u64;
    if next > session.total_bytes {
        return ResponseHelper::json_error("Chunk exceeds the declared file size".to_string());
    }

    // Per-chunk quota re-check against current (possibly updated by other
    // completed transfers) usage.
    let upload_quotas =
        crate::transfer_quota::applicable_upload_quotas(state.db.pool(), &session.username)
            .await
            .unwrap_or_default();
    let _guard = crate::transfer_quota::acquire_quota_locks(&upload_quotas).await;
    if let Err(exceeded) = crate::transfer_quota::check_quota(&upload_quotas, session.total_bytes) {
        return (StatusCode::TOO_MANY_REQUESTS, exceeded.to_string()).into_response();
    }
    drop(_guard);

    let tmp_path = session.tmp_path.clone();
    let write_result: std::io::Result<()> = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&tmp_path)
            .await?;
        file.seek(SeekFrom::Start(params.offset)).await?;
        file.write_all(&body).await?;
        file.flush().await?;
        Ok(())
    }
    .await;
    match write_result {
        Ok(()) => {
            if let Some(s) = UPLOAD_SESSIONS.lock().get_mut(&params.session_id) {
                s.offset = next;
            }
            ResponseHelper::json_success(serde_json::json!({ "offset": next }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to write chunk: {}", e)),
    }
}

/// `POST /api/explorer/upload/complete?session_id=…` — fsync, verify the
/// optional expected SHA-256, resolve the collision strategy, atomically
/// rename the temp file into place, and debit the quota. The quota check is
/// re-run under the per-subject lock right before the debit so a concurrent
/// transfer can't overshoot the cap.
#[axum_macros::debug_handler]
pub async fn handle_explorer_upload_complete(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UploadSessionIdQuery>,
    headers: HeaderMap,
) -> Response {
    prune_upload_sessions();

    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    let session = {
        let map = UPLOAD_SESSIONS.lock();
        match map.get(&params.session_id) {
            Some(s) => s.clone(),
            None => {
                return ResponseHelper::json_error(
                    "Upload session not found or expired".to_string(),
                )
            }
        }
    };

    if session.offset != session.total_bytes {
        return ResponseHelper::json_error(format!(
            "Incomplete upload: {}/{} bytes received",
            session.offset, session.total_bytes
        ));
    }

    let upload_quotas =
        crate::transfer_quota::applicable_upload_quotas(state.db.pool(), &session.username)
            .await
            .unwrap_or_default();
    let guard = crate::transfer_quota::acquire_quota_locks(&upload_quotas).await;
    if let Err(exceeded) = crate::transfer_quota::check_quota(&upload_quotas, session.total_bytes) {
        return (StatusCode::TOO_MANY_REQUESTS, exceeded.to_string()).into_response();
    }

    let tmp_path = session.tmp_path.clone();
    let dir = PathBuf::from(&session.dir);
    let filename = session.filename.clone();
    let expected = session.expected_sha256.clone();
    let strategy = session.strategy;
    let total_bytes = session.total_bytes;
    let tmp_path_for_finalize = tmp_path.clone();
    let filename_for_finalize = filename.clone();

    // Flush + fsync + hash in one blocking step, then resolve the final name.
    let finalize_result =
        tokio::task::spawn_blocking(move || -> Result<(String, PathBuf), String> {
            use sha2::{Digest, Sha256};
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&tmp_path_for_finalize)
                .map_err(|e| format!("Failed to open upload slot: {}", e))?;
            file.sync_all()
                .map_err(|e| format!("Failed to sync upload slot: {}", e))?;

            let mut hasher = Sha256::new();
            let mut reader = std::io::BufReader::new(
                std::fs::File::open(&tmp_path_for_finalize).map_err(|e| e.to_string())?,
            );
            let mut buf = [0u8; 1 << 16];
            loop {
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            let checksum = hasher
                .finalize()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();

            if let Some(expected) = &expected {
                if expected != &checksum {
                    return Err(format!(
                        "Integrity check failed: expected SHA-256 {}, got {}",
                        expected, checksum
                    ));
                }
            }

            let mut file_path = dir.join(&filename_for_finalize);
            match strategy {
                crate::browser::CollisionStrategy::Skip if file_path.exists() => {
                    return Ok((checksum, PathBuf::new())); // signal "skipped"
                }
                crate::browser::CollisionStrategy::KeepBoth => {
                    file_path = crate::browser::non_colliding_path(&file_path);
                }
                _ => {}
            }
            Ok((checksum, file_path))
        })
        .await;

    let (checksum, file_path) = match finalize_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            return ResponseHelper::json_error(e);
        }
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };

    if file_path.as_os_str().is_empty() {
        // Collision strategy said "skip" — nothing to keep.
        let _ = std::fs::remove_file(&tmp_path);
        UPLOAD_SESSIONS.lock().remove(&params.session_id);
        return ResponseHelper::json_success(serde_json::json!({
            "skipped": true,
            "message": "Destination already exists — skipped"
        }));
    }

    if let Err(e) = tokio::fs::rename(&tmp_path, &file_path).await {
        return ResponseHelper::json_error(format!("Failed to finalize upload: {}", e));
    }
    FileBrowser::fsync_parent_dir(&file_path);

    // Only now debit — the file is durable under its final name.
    crate::transfer_quota::record_usage(state.db.pool(), &upload_quotas, total_bytes, true).await;
    drop(guard);
    UPLOAD_SESSIONS.lock().remove(&params.session_id);

    ResponseHelper::json_success(serde_json::json!({
        "filename": file_path.file_name().and_then(|n| n.to_str()).unwrap_or(&filename),
        "checksum": checksum
    }))
}

/// `DELETE /api/explorer/upload/session?session_id=…` — abort a resumable
/// upload and remove its partial temp file.
#[axum_macros::debug_handler]
pub async fn handle_explorer_upload_session_abort(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UploadSessionIdQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }
    let removed = UPLOAD_SESSIONS.lock().remove(&params.session_id);
    if let Some(session) = removed {
        let _ = tokio::fs::remove_file(&session.tmp_path).await;
        return ResponseHelper::json_success(serde_json::json!({ "aborted": true }));
    }
    ResponseHelper::json_error("Upload session not found".to_string())
}

/// Renames a file or directory.
#[axum_macros::debug_handler]
pub async fn handle_explorer_rename(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerRenameQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let (old_path, new_path) = match state.auth.get_session(session_token).await {
        Ok(session) => {
            // Both the source and the destination are full paths — validate
            // each so a rename can't move a file out of the allowed roots
            // (the raw `new_name` was never checked before), and use the
            // canonical validated paths for the rename itself.
            match (
                validate_path_access(&state, &params.path, &session),
                validate_path_access(&state, &params.new_name, &session),
            ) {
                (Some(old_p), Some(new_p)) => (old_p, new_p),
                _ => {
                    tracing::warn!(
                        "Explorer rename blocked: '{}' -> '{}' outside allowed roots",
                        params.path,
                        params.new_name
                    );
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            }
        }
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let path = old_path.to_string_lossy().into_owned();
    let new_name = new_path.to_string_lossy().into_owned();
    match tokio::task::spawn_blocking(move || FileBrowser::rename_item(&path, &new_name)).await {
        Ok(Ok(_)) => {
            FileBrowser::fsync_parent_dir(&new_path);
            ResponseHelper::json_success(
                serde_json::json!({"message": "Item renamed successfully"}),
            )
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to rename item: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// The destination a copy/move/upload should actually write to, after
/// applying the caller's `CollisionStrategy` to a name that already exists.
enum ResolvedDest {
    Write(String),
    /// The destination exists and the strategy asked to skip it.
    Skip,
}

fn resolve_dest(destination: &str, strategy: Option<&str>) -> ResolvedDest {
    use crate::browser::CollisionStrategy;
    let dest = std::path::Path::new(destination);
    match CollisionStrategy::parse(strategy) {
        CollisionStrategy::Overwrite => ResolvedDest::Write(destination.to_string()),
        CollisionStrategy::Skip if dest.exists() => ResolvedDest::Skip,
        CollisionStrategy::Skip => ResolvedDest::Write(destination.to_string()),
        CollisionStrategy::KeepBoth => ResolvedDest::Write(
            crate::browser::non_colliding_path(dest)
                .to_string_lossy()
                .into_owned(),
        ),
    }
}

/// Copies a file or directory from source to destination.
#[axum_macros::debug_handler]
pub async fn handle_explorer_copy(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerCopyQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let (source, destination, username) = match state.auth.get_session(session_token).await {
        Ok(session) => {
            let src_valid = validate_path_access(&state, &params.source, &session);
            let dst_valid = validate_path_access(&state, &params.destination, &session);
            match (src_valid, dst_valid) {
                (Some(src), Some(dst)) => (
                    src.to_string_lossy().into_owned(),
                    dst.to_string_lossy().into_owned(),
                    session.username,
                ),
                _ => {
                    tracing::warn!("Explorer copy blocked: source='{}' or destination='{}' outside allowed roots",
                        params.source, params.destination);
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            }
        }
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let destination = match resolve_dest(&destination, params.strategy.as_deref()) {
        ResolvedDest::Skip => {
            return ResponseHelper::json_success(serde_json::json!({
                "skipped": true,
                "message": "Destination already exists — skipped"
            }));
        }
        ResolvedDest::Write(d) => d,
    };

    // Directories go through the tracked-progress job path (see
    // `spawn_tracked_copy_job`) since a large tree can take a long time;
    // single files stay on the fast synchronous path.
    if std::path::Path::new(&source).is_dir() {
        let (job_id, sse_url) = spawn_tracked_copy_job(source, destination, username);
        return ResponseHelper::json_success(serde_json::json!({
            "job_id": job_id,
            "sse_url": sse_url,
            "message": "Copy started"
        }));
    }

    match tokio::task::spawn_blocking(move || FileBrowser::copy_item(&source, &destination)).await {
        Ok(Ok(_)) => {
            ResponseHelper::json_success(serde_json::json!({"message": "Item copied successfully"}))
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to copy item: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Moves a file or directory. Falls back to copy+delete for cross-filesystem moves.
#[axum_macros::debug_handler]
pub async fn handle_explorer_move(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerMoveQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let (source, destination, username) = match state.auth.get_session(session_token).await {
        Ok(session) => {
            let src_valid = validate_path_access(&state, &params.source, &session);
            let dst_valid = validate_path_access(&state, &params.destination, &session);
            match (src_valid, dst_valid) {
                (Some(src), Some(dst)) => (
                    src.to_string_lossy().into_owned(),
                    dst.to_string_lossy().into_owned(),
                    session.username,
                ),
                _ => {
                    tracing::warn!("Explorer move blocked: source='{}' or destination='{}' outside allowed roots",
                        params.source, params.destination);
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            }
        }
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let destination = match resolve_dest(&destination, params.strategy.as_deref()) {
        ResolvedDest::Skip => {
            return ResponseHelper::json_success(serde_json::json!({
                "skipped": true,
                "message": "Destination already exists — skipped"
            }));
        }
        ResolvedDest::Write(d) => d,
    };

    // For directories, try the atomic rename first (instant regardless of
    // size on the same filesystem); only fall back to the tracked-progress
    // copy+delete job (see `spawn_tracked_move_job`) if that fails, e.g.
    // cross-filesystem.
    if std::path::Path::new(&source).is_dir() {
        let rename_src = source.clone();
        let rename_dst = destination.clone();
        let rename_result =
            tokio::task::spawn_blocking(move || std::fs::rename(&rename_src, &rename_dst)).await;

        if let Ok(Ok(())) = rename_result {
            return ResponseHelper::json_success(
                serde_json::json!({"message": "Item moved successfully"}),
            );
        }

        tracing::warn!("Atomic directory move failed (cross-filesystem?), falling back to a tracked copy+delete job");
        let (job_id, sse_url) = spawn_tracked_move_job(source, destination, username);
        return ResponseHelper::json_success(serde_json::json!({
            "job_id": job_id,
            "sse_url": sse_url,
            "message": "Move started"
        }));
    }

    // Move using atomic rename, falling back to verified-copy-then-delete
    // for a cross-filesystem source/destination — `FileBrowser::move_item`
    // already implements that whole cascade itself (including the
    // integrity-verified copy that must succeed before it ever deletes the
    // source), so it must not be retried again here: a second blind
    // copy+delete on any error would silently retry past a genuine
    // `IntegrityMismatch` (real data corruption caught by the hash check)
    // as if it were a routine cross-device fallback, hiding a corruption
    // event from the caller whenever the retry happened to succeed.
    let result = tokio::task::spawn_blocking(move || {
        FileBrowser::move_item(&source, &destination).map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(_)) => {
            ResponseHelper::json_success(serde_json::json!({"message": "Item moved successfully"}))
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Returns a text preview of a file (first 10 MB).
#[axum_macros::debug_handler]
pub async fn handle_explorer_preview(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerPreviewQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            let validated = match validate_path_access(&state, &params.path, &session) {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "Explorer preview blocked: path '{}' outside allowed roots",
                        params.path
                    );
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            };
            let path = validated.to_string_lossy().into_owned();
            let result = tokio::task::spawn_blocking(move || {
                FileBrowser::preview_file(&path, 10 * 1024 * 1024)
            })
            .await;

            match result {
                Ok(Ok(preview)) => ResponseHelper::json_success(preview),
                Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to preview file: {}", e)),
                Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
            }
        }
        Err(_) => ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }
}

/// Gets or sets Unix file permissions (chmod).
#[axum_macros::debug_handler]
pub async fn handle_explorer_permissions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerPermissionsQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let validated_path = match state.auth.get_session(session_token).await {
        Ok(session) => match validate_path_access(&state, &params.path, &session) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Explorer permissions blocked: path '{}' outside allowed roots",
                    params.path
                );
                return ResponseHelper::json_error(
                    "Access denied: path outside allowed directory".to_string(),
                );
            }
        },
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    // If mode is provided, try to change permissions
    if let Some(mode) = params.mode {
        // Validate mode format: must be 3 or 4 octal digits (e.g. "755", "0644")
        if mode.len() < 3 || mode.len() > 4 || !mode.chars().all(|c| ('0'..='7').contains(&c)) {
            tracing::warn!("Explorer permissions blocked: invalid mode '{}'", mode);
            return ResponseHelper::json_error(
                "Invalid mode format: must be 3-4 octal digits (e.g. 755, 0644)".to_string(),
            );
        }

        let path = validated_path.to_string_lossy().into_owned();
        let result = tokio::task::spawn_blocking(move || {
            use std::process::Command;
            Command::new("chmod").arg(&mode).arg(&path).output()
        })
        .await;

        match result {
            Ok(Ok(_)) => ResponseHelper::json_success(
                serde_json::json!({"message": "Permissions changed successfully"}),
            ),
            Ok(Err(e)) => {
                ResponseHelper::json_error(format!("Failed to change permissions: {}", e))
            }
            Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
        }
    } else {
        // Just return current permissions
        let path = validated_path.to_string_lossy().into_owned();
        let result = tokio::task::spawn_blocking(move || {
            std::fs::metadata(&path).map(|metadata| FileBrowser::format_permissions(&metadata))
        })
        .await;

        match result {
            Ok(Ok(permissions)) => {
                ResponseHelper::json_success(serde_json::json!({"permissions": permissions}))
            }
            Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to get permissions: {}", e)),
            Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
        }
    }
}

/// Extracts an archive (zip, tar, tar.gz, 7z, etc.) to a destination directory.
#[axum_macros::debug_handler]
pub async fn handle_explorer_extract(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerExtractQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let (archive_path, destination) = match state.auth.get_session(session_token).await {
        Ok(session) => {
            let path_valid = validate_path_access(&state, &params.path, &session);
            let dst_valid = validate_path_access(&state, &params.destination, &session);
            match (path_valid, dst_valid) {
                (Some(p), Some(d)) => (
                    p.to_string_lossy().into_owned(),
                    d.to_string_lossy().into_owned(),
                ),
                _ => {
                    tracing::warn!("Explorer extract blocked: path='{}' or destination='{}' outside allowed roots",
                        params.path, params.destination);
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            }
        }
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let result = tokio::task::spawn_blocking(move || -> Result<std::process::Output, String> {
        use std::process::Command;

        // Determine file type and extract accordingly
        let file_path = std::path::Path::new(&archive_path);
        let extension = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let full_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_tar_gz = full_name.ends_with(".tar.gz") || full_name.ends_with(".tgz");

        // The external tool each format needs — checked with `which` first so a
        // missing tool produces a clear message instead of a raw exec error.
        let required_tool = if is_tar_gz {
            "tar"
        } else {
            match extension {
                "bz2" | "xz" | "tar" => "tar",
                "zip" => "unzip",
                "7z" => "7z",
                "gz" => "gunzip",
                _ => "unzip",
            }
        };
        if Command::new("which")
            .arg(required_tool)
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            return Err(format!(
                "Cannot extract this archive: '{}' is not installed on this server",
                required_tool
            ));
        }

        // Detect format by extension or filename pattern
        let result = if is_tar_gz {
            // tar.gz and .tgz
            Command::new("tar")
                .arg("-xzf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&destination)
                .output()
        } else {
            match extension {
                // bz2
                "bz2" => Command::new("tar")
                    .arg("-xjf")
                    .arg(&archive_path)
                    .arg("-C")
                    .arg(&destination)
                    .output(),
                // xz
                "xz" => Command::new("tar")
                    .arg("-xJf")
                    .arg(&archive_path)
                    .arg("-C")
                    .arg(&destination)
                    .output(),
                // zip
                "zip" => Command::new("unzip")
                    .arg("-o")
                    .arg(&archive_path)
                    .arg("-d")
                    .arg(&destination)
                    .output(),
                // 7z
                "7z" => Command::new("7z")
                    .arg("x")
                    .arg(&archive_path)
                    .arg(format!("-o{}", destination))
                    .output(),
                // tar (no compression)
                "tar" => Command::new("tar")
                    .arg("-xf")
                    .arg(&archive_path)
                    .arg("-C")
                    .arg(&destination)
                    .output(),
                // gzip single file
                "gz" => {
                    let stem = file_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("output");
                    let output_file = std::path::Path::new(&destination).join(stem);
                    Command::new("gunzip")
                        .arg("-c")
                        .arg(&archive_path)
                        .output()
                        .and_then(|out| {
                            let stdout = out.stdout.clone();
                            std::fs::write(&output_file, stdout)?;
                            Ok(std::process::Output {
                                status: out.status,
                                stdout: out.stdout,
                                stderr: out.stderr,
                            })
                        })
                }
                // Default: try unzip
                _ => Command::new("unzip")
                    .arg("-o")
                    .arg(&archive_path)
                    .arg("-d")
                    .arg(&destination)
                    .output(),
            }
        };

        result.map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(_)) => ResponseHelper::json_success(
            serde_json::json!({"message": "Archive extracted successfully"}),
        ),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to extract archive: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Maximum edge length (px) of a generated thumbnail. Bounds both the
/// response payload sent to the browser and (indirectly) how much of the
/// decoded bitmap survives past the resize step.
const THUMBNAIL_MAX_EDGE: u32 = 256;

/// Decodes an image file and returns a small PNG thumbnail. Runs on a
/// blocking-pool thread since `image` has no bounded/progressive decoder —
/// the full original file still has to be read and decoded before it can be
/// downscaled, so this must never run directly on a Tokio worker thread.
fn generate_thumbnail(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let thumb = img.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE);
    let mut buf = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Returns a downscaled thumbnail for an image file (SVGs are served as-is —
/// they're already lightweight vectors and `image` can't rasterize them).
///
/// Authorisation is either a multi-use, directory-scoped thumbnail ticket
/// (`?ticket=…`, minted by `POST /api/explorer/thumbnail-ticket`) or the
/// `x-session-token` header. The session token is never accepted in the URL.
#[axum_macros::debug_handler]
pub async fn handle_explorer_thumbnail(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerThumbnailQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let (file_path, _username) = if let Some(ticket) = params.ticket.as_deref() {
        match crate::media_ticket::validate_thumbnail_ticket(ticket, &params.path) {
            Some((username, canonical)) => (PathBuf::from(canonical), username),
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    "Invalid or expired thumbnail ticket".to_string(),
                )
                    .into_response();
            }
        }
    } else {
        let session_token = headers
            .get("x-session-token")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if session_token.is_empty() {
            return (
                StatusCode::UNAUTHORIZED,
                "No session token provided".to_string(),
            )
                .into_response();
        }
        let session = match state.auth.get_session(session_token).await {
            Ok(s) => s,
            Err(_) => {
                return (StatusCode::UNAUTHORIZED, "Session invalid".to_string()).into_response();
            }
        };
        // Validate path is within allowed roots
        let Some(validated) = validate_path_access(&state, &params.path, &session) else {
            tracing::warn!(
                "Explorer thumbnail blocked: path '{}' outside allowed roots",
                params.path
            );
            return (
                StatusCode::FORBIDDEN,
                "Access denied: path outside allowed directory".to_string(),
            )
                .into_response();
        };
        (validated, session.username)
    };

    if !file_path.exists() {
        return (StatusCode::NOT_FOUND, "File not found".to_string()).into_response();
    }

    // Check if it's an image file
    let extension = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_image = matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
    );

    if !is_image {
        return (StatusCode::BAD_REQUEST, "Not an image file".to_string()).into_response();
    }

    // SVGs are already small vectors — serve them directly rather than
    // trying to rasterize them.
    if extension == "svg" {
        return match std::fs::read(&file_path) {
            Ok(contents) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "image/svg+xml")
                .header("Cache-Control", "public, max-age=3600")
                .body(Body::from(contents))
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("Failed to send image"))
                        .unwrap()
                }),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read file: {}", e),
            )
                .into_response(),
        };
    }

    let path = file_path;
    let thumbnail = tokio::task::spawn_blocking(move || generate_thumbnail(&path)).await;

    match thumbnail {
        Ok(Ok(contents)) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "image/png")
            .header("Cache-Control", "public, max-age=3600")
            .body(Body::from(contents))
            .unwrap_or_else(|_| {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from("Failed to send thumbnail"))
                    .unwrap()
            }),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to generate thumbnail: {}", e),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Internal error: {}", e),
        )
            .into_response(),
    }
}

/// `POST /api/explorer/thumbnail-ticket` — validates that the caller can
/// browse the given directory, then mints a multi-use ticket scoped to it so
/// every `<img>` in that listing can be thumbnailed without putting the
/// session token in a URL.
#[axum_macros::debug_handler]
pub async fn handle_explorer_thumbnail_ticket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ExplorerPreviewQuery>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }
    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let Some(dir) = validate_path_access(&state, &params.path, &session) else {
        tracing::warn!(
            "Thumbnail ticket blocked: path '{}' outside allowed roots",
            params.path
        );
        return ResponseHelper::json_forbidden(
            "Access denied: path outside allowed directory".to_string(),
        );
    };
    if !dir.is_dir() {
        return ResponseHelper::json_error("Ticket scope must be a directory".to_string());
    }
    let canonical = std::fs::canonicalize(&dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dir.to_string_lossy().into_owned());
    let ticket = crate::media_ticket::issue_thumbnail_ticket(
        &session.username,
        &canonical,
        session.is_admin,
    );
    ResponseHelper::json_success(serde_json::json!({ "ticket": ticket }))
}

/// Strips a trailing partition number from a bare device name (no `/dev/`
/// prefix), so a partition can be traced back to its parent block device in
/// `/sys/block`. NVMe/mmcblk devices separate the partition with a 'p'
/// (nvme0n1p2 -> nvme0n1); SATA/SCSI/virtio devices don't (sda1 -> sda).
fn strip_partition_suffix(name: &str) -> String {
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        if let Some(pos) = name.rfind('p') {
            let after = &name[pos + 1..];
            if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
                return name[..pos].to_string();
            }
        }
        name.to_string()
    } else {
        name.trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string()
    }
}

/// Determines whether a `/dev/...` block device is removable/external (USB,
/// SD card, etc.), rather than guessing from the device path — a real
/// USB-attached drive is named `/dev/sdX` exactly like an internal SATA
/// disk, so there is no substring in the path itself that reliably
/// indicates "external".
///
/// Two signals are checked because neither is reliable alone: the kernel's
/// `/sys/block/<disk>/removable` flag, and whether the device's real sysfs
/// path passes through a USB bus node. Confirmed against real hardware that
/// a USB-to-SATA bridge can report `removable=0` (and even
/// `ID_BUS=ata` via udev) while its sysfs devpath still resolves through
/// `.../usbN/...` — the physical bus topology is the one signal a bridge
/// chip can't hide.
fn is_removable_block_device(device: &str) -> bool {
    let Some(name) = device.strip_prefix("/dev/") else {
        return false;
    };
    let disk = strip_partition_suffix(name);

    let removable_flag = std::fs::read_to_string(format!("/sys/block/{}/removable", disk))
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    if removable_flag {
        return true;
    }

    std::fs::canonicalize(format!("/sys/class/block/{}", disk))
        .map(|resolved| {
            resolved
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with("usb"))
        })
        .unwrap_or(false)
}

/// Returns mounted filesystems categorized by type (system, internal, raid, external, cloud, network).
#[axum_macros::debug_handler]
pub async fn handle_explorer_disks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(_) => {}
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }

    let mut response = DisksResponse {
        system: vec![],
        internal: vec![],
        raid: vec![],
        external: vec![],
        cloud: vec![],
        network: vec![],
    };

    // One lsblk pass for every mount's label + parent-disk make/model/serial.
    let block_meta = collect_block_device_meta();

    // Read /proc/mounts to get mounted filesystems
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let device = parts[0];
                let mount_point = parts[1];
                let filesystem = parts[2];

                // Skip virtual filesystems
                if filesystem == "proc"
                    || filesystem == "sysfs"
                    || filesystem == "devtmpfs"
                    || filesystem == "tmpfs"
                    || filesystem == "cgroup"
                    || filesystem == "cgroup2"
                    || filesystem == "pstore"
                    || filesystem == "efivarfs"
                    || filesystem == "configfs"
                    || filesystem == "debugfs"
                    || filesystem == "tracefs"
                    || filesystem == "hugetlbfs"
                    || filesystem == "mqueue"
                    || filesystem == "securityfs"
                {
                    continue;
                }

                // Get disk usage
                let (size_bytes, used_bytes) = match std::fs::metadata(mount_point) {
                    Ok(_) => {
                        use std::process::Command;
                        match Command::new("df").arg("-B1").arg(mount_point).output() {
                            Ok(output) => {
                                let output_str = String::from_utf8_lossy(&output.stdout);
                                let lines: Vec<&str> = output_str.lines().collect();
                                if lines.len() >= 2 {
                                    let parts: Vec<&str> = lines[1].split_whitespace().collect();
                                    let total = parts
                                        .get(1)
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .unwrap_or(0);
                                    let used = parts
                                        .get(2)
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .unwrap_or(0);
                                    (total, used)
                                } else {
                                    (0, 0)
                                }
                            }
                            Err(_) => (0, 0),
                        }
                    }
                    Err(_) => (0, 0),
                };

                // Determine category
                let category = if device.starts_with("/dev/sd")
                    || device.starts_with("/dev/nvme")
                    || device.starts_with("/dev/mmc")
                {
                    if mount_point == "/" || mount_point == "/boot" || mount_point == "/boot/efi" {
                        "system".to_string()
                    } else if device.contains("md") || device.contains("raid") {
                        "raid".to_string()
                    } else if is_removable_block_device(device) {
                        "external".to_string()
                    } else {
                        "internal".to_string()
                    }
                } else if device.starts_with("rclone") || filesystem == "rclone" {
                    // Only show rclone mounts in the default directory
                    if mount_point.starts_with(crate::config::paths::RCLONE_MOUNT_BASE)
                        || mount_point.starts_with("/run/rclone")
                    {
                        "cloud".to_string()
                    } else {
                        "internal".to_string()
                    }
                } else if device.starts_with("//")
                    || filesystem == "cifs"
                    || filesystem == "smb"
                    || filesystem == "nfs"
                    || filesystem == "nfs4"
                    || filesystem == "nfs3"
                {
                    "network".to_string()
                } else {
                    "internal".to_string()
                };

                let meta = block_meta.get(device).cloned().unwrap_or_default();
                let disk_info = DiskInfo {
                    name: device.to_string(),
                    path: device.to_string(),
                    mount_point: mount_point.to_string(),
                    filesystem: filesystem.to_string(),
                    size_bytes,
                    used_bytes,
                    category: category.clone(),
                    label: meta.label,
                    vendor: meta.vendor,
                    model: meta.model,
                    serial: meta.serial,
                };

                match category.as_str() {
                    "system" => response.system.push(disk_info),
                    "internal" => response.internal.push(disk_info),
                    "raid" => response.raid.push(disk_info),
                    "external" => response.external.push(disk_info),
                    "cloud" => response.cloud.push(disk_info),
                    "network" => response.network.push(disk_info),
                    _ => response.internal.push(disk_info),
                }
            }
        }
    }

    ResponseHelper::json_success(response)
}

#[derive(Debug, Deserialize)]
pub struct ExplorerCompressRequest {
    pub paths: Vec<String>,
    pub destination: String,
    /// `"zip"` or anything else for the default `tar.gz`.
    pub format: Option<String>,
}

/// Creates an archive from one or more selected items. Defaults to tar.gz
/// (always available via core utils); zip is offered only when a `zip`
/// binary is actually detected, mirroring the tool-detection pattern used
/// by `handle_explorer_extract`.
#[axum_macros::debug_handler]
pub async fn handle_explorer_compress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<ExplorerCompressRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let (paths, destination) = match state.auth.get_session(session_token).await {
        Ok(session) => {
            let dest_valid = validate_path_access(&state, &payload.destination, &session);
            let mut sources_valid: Vec<PathBuf> = Vec::with_capacity(payload.paths.len());
            for p in &payload.paths {
                match validate_path_access(&state, p, &session) {
                    Some(canonical) => sources_valid.push(canonical),
                    None => {
                        tracing::warn!(
                            "Explorer compress blocked: source '{}' outside allowed roots",
                            p
                        );
                        return ResponseHelper::json_error(
                            "Access denied: path outside allowed directory".to_string(),
                        );
                    }
                }
            }
            let Some(dst) = dest_valid else {
                tracing::warn!(
                    "Explorer compress blocked: destination='{}' outside allowed roots",
                    payload.destination
                );
                return ResponseHelper::json_error(
                    "Access denied: path outside allowed directory".to_string(),
                );
            };
            (
                sources_valid
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<String>>(),
                dst.to_string_lossy().into_owned(),
            )
        }
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let format = payload.format.unwrap_or_else(|| "tar.gz".to_string());

    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        if format == "zip" {
            let has_zip = std::process::Command::new("which")
                .arg("zip")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !has_zip {
                return Err(
                    "Cannot create a .zip archive: 'zip' is not installed on this server"
                        .to_string(),
                );
            }
        }
        FileBrowser::compress_items(&paths, &destination, &format).map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(_)) => ResponseHelper::json_success(
            serde_json::json!({"message": "Archive created successfully"}),
        ),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to create archive: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Moves a file or directory to the trash instead of permanently deleting
/// it. This is what the Explorer UI's "Delete" action calls; the real
/// permanent delete (`handle_explorer_delete`, above) is now only used
/// internally by the purge/empty-trash handlers below.
#[axum_macros::debug_handler]
pub async fn handle_explorer_trash(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerDeleteQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            let validated = match validate_path_access(&state, &params.path, &session) {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "Explorer trash blocked: path '{}' outside allowed roots",
                        params.path
                    );
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            };
            let trash_root = resolve_trash_root(&state, &validated.to_string_lossy());
            let path = validated.to_string_lossy().into_owned();
            let result =
                tokio::task::spawn_blocking(move || FileBrowser::trash_item(&path, &trash_root))
                    .await;

            match result {
                Ok(Ok(_)) => ResponseHelper::json_success(
                    serde_json::json!({"message": "Item moved to trash"}),
                ),
                Ok(Err(e)) => {
                    ResponseHelper::json_error(format!("Failed to move item to trash: {}", e))
                }
                Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
            }
        }
        Err(_) => ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }
}

/// Lists everything in every trash directory this session can see.
#[axum_macros::debug_handler]
pub async fn handle_explorer_trash_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let roots = all_trash_roots(&state, session.is_admin);
    let result = tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        for root in roots {
            entries.extend(FileBrowser::list_trash(&root));
        }
        entries
    })
    .await;

    match result {
        Ok(entries) => ResponseHelper::json_success(entries),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ExplorerTrashIdQuery {
    pub id: String,
}

/// Restores a trashed item back to its original location.
#[axum_macros::debug_handler]
pub async fn handle_explorer_trash_restore(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerTrashIdQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let roots = all_trash_roots(&state, session.is_admin);
    let id = params.id.clone();
    let result = tokio::task::spawn_blocking(move || {
        for root in roots {
            match FileBrowser::restore_trash_item(&root, &id) {
                Ok(_) => return Ok(()),
                Err(crate::browser::BrowserError::NotFound(_)) => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Trash entry not found".to_string())
    })
    .await;

    match result {
        Ok(Ok(_)) => ResponseHelper::json_success(serde_json::json!({"message": "Item restored"})),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to restore item: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Permanently deletes a single trashed item.
#[axum_macros::debug_handler]
pub async fn handle_explorer_trash_purge(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let roots = all_trash_roots(&state, session.is_admin);
    let result = tokio::task::spawn_blocking(move || {
        for root in roots {
            match FileBrowser::purge_trash_item(&root, &id) {
                Ok(_) => return Ok(()),
                Err(crate::browser::BrowserError::NotFound(_)) => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Trash entry not found".to_string())
    })
    .await;

    match result {
        Ok(Ok(_)) => {
            ResponseHelper::json_success(serde_json::json!({"message": "Item permanently deleted"}))
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to purge item: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Permanently empties every trash directory this session can see.
#[axum_macros::debug_handler]
pub async fn handle_explorer_trash_empty(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let roots = all_trash_roots(&state, session.is_admin);
    let result = tokio::task::spawn_blocking(move || {
        for root in &roots {
            FileBrowser::empty_trash(root)?;
        }
        Ok::<(), crate::browser::BrowserError>(())
    })
    .await;

    match result {
        Ok(Ok(_)) => ResponseHelper::json_success(serde_json::json!({"message": "Trash emptied"})),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to empty trash: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Inline text/code editor
// ---------------------------------------------------------------------------

/// Largest file the inline editor will save — mirrors the 5 MB the browser
/// realistically wants to hold in a `<textarea>` (and stays well under the
/// 10 MB preview cap).
const MAX_EDIT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct ExplorerSaveRequest {
    pub path: String,
    pub content: String,
}

/// Overwrites a text file's contents (the Explorer's inline editor). Writes
/// through a sibling temp file + atomic rename, exactly like
/// `handle_explorer_upload`, so a crash mid-write never truncates the
/// original.
#[axum_macros::debug_handler]
pub async fn handle_explorer_save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<ExplorerSaveRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let target = match state.auth.get_session(session_token).await {
        Ok(session) => match validate_path_access(&state, &payload.path, &session) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Explorer save blocked: path '{}' outside allowed roots",
                    payload.path
                );
                return ResponseHelper::json_error(
                    "Access denied: path outside allowed directory".to_string(),
                );
            }
        },
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if payload.content.len() > MAX_EDIT_BYTES {
        return ResponseHelper::json_error("File is too large to edit here".to_string());
    }

    if target.is_dir() {
        return ResponseHelper::json_error("Cannot write to a directory".to_string());
    }
    let parent = match target.parent() {
        Some(p) => p.to_path_buf(),
        None => return ResponseHelper::json_error("Invalid path".to_string()),
    };
    if !parent.is_dir() {
        return ResponseHelper::json_error("Parent directory does not exist".to_string());
    }

    let content = payload.content.clone();
    let result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let base = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let tmp = parent.join(format!(".{}.hvsave-{}", base, uuid::Uuid::new_v4()));
        std::fs::write(&tmp, content.as_bytes())?;
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        FileBrowser::fsync_parent_dir(&target);
        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => ResponseHelper::json_success(serde_json::json!({"message": "File saved"})),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to save file: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Labelled sidebar roots ("virtual roots")
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ExplorerRoot {
    /// Absolute path this entry opens.
    pub path: String,
    /// A stable machine key the frontend maps to a translated label
    /// (`explorer.root_<kind>`); `storage` has no key and falls back to `name`.
    pub kind: String,
    /// Last path segment — the label fallback when there is no i18n key.
    pub name: String,
}

/// Classifies an allowed root into a `kind` key for the UI.
fn kind_for_root(path: &str) -> &'static str {
    match path {
        "/home" => "users",
        "/srv" => "shares",
        "/media" | "/mnt" => "media",
        "/" => "system",
        p if p.contains("homevault") => "appdata",
        _ => "storage",
    }
}

/// The labelled starting points shown at the top of the Explorer sidebar:
/// the signed-in user's own home directory first, then every configured
/// allowed root with a friendly `kind`.
#[axum_macros::debug_handler]
pub async fn handle_explorer_roots(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let mut roots: Vec<ExplorerRoot> = Vec::new();

    let home = format!("/home/{}", session.username);
    if std::path::Path::new(&home).is_dir() {
        roots.push(ExplorerRoot {
            path: home,
            kind: "home".to_string(),
            name: session.username.clone(),
        });
    }

    for root in state.path_guard.allowed_roots() {
        let p = root.to_string_lossy().to_string();
        if roots.iter().any(|r| r.path == p) {
            continue;
        }
        let name = p
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(p.as_str())
            .to_string();
        roots.push(ExplorerRoot {
            kind: kind_for_root(&p).to_string(),
            path: p,
            name,
        });
    }

    // Every mounted virtual disk (mdadm / ZFS / Btrfs) as its own entry,
    // labelled by the disk's name rather than a raw device path.
    if let Ok(pools) = tokio::task::spawn_blocking(crate::pools::PoolService::scan_pools).await {
        for pool in pools.into_iter().flatten() {
            if let Some(mp) = pool.mount_point {
                if mp.is_empty() || roots.iter().any(|r| r.path == mp) {
                    continue;
                }
                let name = pool
                    .name
                    .trim_start_matches("/dev/md/")
                    .trim_start_matches("/dev/")
                    .to_string();
                roots.push(ExplorerRoot {
                    path: mp,
                    kind: "virtualdisk".to_string(),
                    name,
                });
            }
        }
    }

    ResponseHelper::json_success(roots)
}

// ---------------------------------------------------------------------------
// Safe-eject an external / removable drive
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ExplorerEjectQuery {
    /// The drive's mount point (as shown in the sidebar).
    pub path: String,
}

/// Unmounts a removable/external drive and cuts its bus power so it's safe
/// to physically remove. Verifies the target really is a removable block
/// device's mount point first — this can never touch an internal disk, a
/// RAID array or a storage pool.
#[axum_macros::debug_handler]
pub async fn handle_explorer_eject(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerEjectQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) if session.is_admin => {}
        Ok(_) => return ResponseHelper::json_forbidden("Admin required".to_string()),
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }

    let mount_point = params.path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mounts = std::fs::read_to_string("/proc/mounts").map_err(|e| e.to_string())?;
        let device = mounts.lines().find_map(|line| {
            let mut it = line.split_whitespace();
            let dev = it.next()?;
            let mp = it.next()?;
            (mp == mount_point).then(|| dev.to_string())
        });
        let device = match device {
            Some(d) => d,
            None => return Err(format!("'{}' is not a mount point", mount_point)),
        };
        if !is_removable_block_device(&device) {
            return Err("Not an external or removable drive".to_string());
        }

        let out = std::process::Command::new("sudo")
            .arg("umount")
            .arg(&mount_point)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(if msg.trim().is_empty() {
                "umount failed".to_string()
            } else {
                msg.trim().to_string()
            });
        }

        // Best effort: power the device down so it's safe to unplug.
        let bare = device.strip_prefix("/dev/").unwrap_or(&device);
        let disk = strip_partition_suffix(bare);
        let _ = std::process::Command::new("sudo")
            .arg("udisksctl")
            .arg("power-off")
            .arg("-b")
            .arg(format!("/dev/{}", disk))
            .output();
        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => ResponseHelper::json_success(serde_json::json!({"message": "Drive ejected"})),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to eject: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Browse a past ZFS snapshot ("time travel")
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ExplorerSnapshotsQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ExplorerSnapshot {
    /// Bare snapshot name (the part after `@`).
    pub name: String,
    /// Creation time, Unix epoch seconds (as a string).
    pub created: String,
    /// Read-only path the Explorer can navigate into to see `path` as it
    /// was in this snapshot (`<mount>/.zfs/snapshot/<name>/<relative>`).
    pub browse_path: String,
}

/// Lists the ZFS snapshots covering `path` (via the always-present
/// `.zfs/snapshot` pseudo-directory). Btrfs / restic history is not exposed
/// here — ZFS is the one with a stable in-tree browse path.
#[axum_macros::debug_handler]
pub async fn handle_explorer_snapshots(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExplorerSnapshotsQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            let validated = match validate_path_access(&state, &params.path, &session) {
                Some(p) => p,
                None => {
                    return ResponseHelper::json_error(
                        "Access denied: path outside allowed directory".to_string(),
                    );
                }
            };
            let target = validated.to_string_lossy().into_owned();
            render_snapshots(target).await
        }
        Err(_) => ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }
}

/// Runs the ZFS snapshot listing for a validated target path and builds the
/// JSON response.
async fn render_snapshots(target: String) -> Response {
    let snaps = tokio::task::spawn_blocking(move || -> Vec<ExplorerSnapshot> {
        let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
            return Vec::new();
        };

        // The deepest ZFS mount point that contains `target`.
        let mut best: Option<(String, String)> = None; // (mountpoint, dataset)
        for line in mounts.lines() {
            let mut it = line.split_whitespace();
            let (Some(dev), Some(mp), Some(fs)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if fs != "zfs" {
                continue;
            }
            let mp_trim = mp.trim_end_matches('/');
            if target == mp || target.starts_with(&format!("{}/", mp_trim)[..]) {
                let deeper = best
                    .as_ref()
                    .map(|(m, _)| mp.len() > m.len())
                    .unwrap_or(true);
                if deeper {
                    best = Some((mp.to_string(), dev.to_string()));
                }
            }
        }

        let Some((mountpoint, dataset)) = best else {
            return Vec::new();
        };
        let mp_trim = mountpoint.trim_end_matches('/').to_string();
        let rel = target
            .strip_prefix(mp_trim.as_str())
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();

        let out = std::process::Command::new("sudo")
            .args([
                "zfs",
                "list",
                "-H",
                "-p",
                "-t",
                "snapshot",
                "-o",
                "name,creation",
                "-r",
                "-d",
                "1",
            ])
            .arg(&dataset)
            .output();
        let Ok(out) = out else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }

        let mut snaps: Vec<ExplorerSnapshot> = Vec::new();
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            let mut cols = line.split('\t');
            let (Some(full), Some(creation)) = (cols.next(), cols.next()) else {
                continue;
            };
            let snapname = full.rsplit('@').next().unwrap_or(full);
            let mut browse = format!("{}/.zfs/snapshot/{}", mp_trim, snapname);
            if !rel.is_empty() {
                browse = format!("{}/{}", browse, rel);
            }
            snaps.push(ExplorerSnapshot {
                name: snapname.to_string(),
                created: creation.to_string(),
                browse_path: browse,
            });
        }
        snaps.reverse(); // newest first
        snaps
    })
    .await
    .unwrap_or_default();

    ResponseHelper::json_success(snaps)
}

// ---------------------------------------------------------------------------
// Explorer favorites (server-side, per-user)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FavoriteRequest {
    pub path: String,
    pub label: Option<String>,
}

/// GET /api/explorer/favorites — list the current user's favorites.
pub async fn handle_explorer_favorites_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_error("Session invalid".to_string()),
    };

    let repo = crate::storage::FavoriteRepository::new(state.db.pool());
    match repo.list(&session.username).await {
        Ok(favorites) => ResponseHelper::json_success(favorites),
        Err(e) => ResponseHelper::json_error(format!("Failed to list favorites: {}", e)),
    }
}

/// POST /api/explorer/favorites — add a favorite for the current user.
pub async fn handle_explorer_favorites_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<FavoriteRequest>,
) -> impl IntoResponse {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_error("Session invalid".to_string()),
    };

    if req.path.is_empty() || req.path == "/" {
        return ResponseHelper::json_error("Invalid path".to_string());
    }

    // Favorites must be pinned inside paths the user can actually browse.
    let stored_path = match validate_path_access(&state, &req.path, &session) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => {
            return ResponseHelper::json_error(
                "Access denied: path outside allowed directory".to_string(),
            );
        }
    };

    let label = req.label.unwrap_or_else(|| {
        req.path
            .split('/')
            .rfind(|s| !s.is_empty())
            .unwrap_or(&req.path)
            .to_string()
    });

    let repo = crate::storage::FavoriteRepository::new(state.db.pool());
    match repo.add(&session.username, &stored_path, &label).await {
        Ok(fav) => ResponseHelper::json_success(fav),
        Err(e) => ResponseHelper::json_error(format!("Failed to add favorite: {}", e)),
    }
}

/// DELETE /api/explorer/favorites — remove a favorite for the current user.
pub async fn handle_explorer_favorites_remove(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<FavoriteRequest>,
) -> impl IntoResponse {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_error("Session invalid".to_string()),
    };

    let repo = crate::storage::FavoriteRepository::new(state.db.pool());
    match repo.remove(&session.username, &req.path).await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({"removed": true})),
        Err(crate::storage::database::DbError::NotFound) => {
            ResponseHelper::json_error("Favorite not found".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove favorite: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Explorer filesystem watcher (real-time SSE)
// ---------------------------------------------------------------------------

/// POST /api/explorer/watch/ticket — issue a short-lived, single-use ticket
/// for the SSE connection (EventSource cannot set custom headers).
pub async fn handle_explorer_watch_ticket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return response;
    }

    let ticket = crate::sse::file_watcher::issue_ticket();
    ResponseHelper::json_success(serde_json::json!({ "ticket": ticket }))
}

#[derive(Debug, Deserialize)]
pub struct WatchQuery {
    pub ticket: String,
    pub path: String,
}

/// GET /api/explorer/watch?ticket=...&path=... — SSE stream of filesystem
/// events for the given directory.
pub async fn handle_explorer_watch(Query(query): Query<WatchQuery>) -> Response {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;
    use std::time::Duration;

    if !crate::sse::file_watcher::consume_ticket(&query.ticket) {
        return ResponseHelper::json_error("Invalid or expired ticket".to_string());
    }

    let watch_path = std::path::PathBuf::from(&query.path);
    crate::sse::file_watcher::watch_path(&watch_path);

    let mut rx = crate::sse::file_watcher::FILE_CHANGE_BUS.subscribe();
    let path_prefix = watch_path.to_string_lossy().into_owned();

    let stream = async_stream::stream! {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(change) => {
                            let relevant = change.paths.iter().any(|p| p.starts_with(&path_prefix));
                            if !relevant { continue; }
                            let json = serde_json::to_string(&change).unwrap_or_default();
                            yield Ok::<_, Infallible>(Event::default().event("change").data(json));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            continue;
                        }
                        Err(_) => break,
                    }
                }
                _ = ticker.tick() => {
                    yield Ok::<_, Infallible>(Event::default().event("heartbeat").data("{}"));
                }
            }
        }
    };

    Sse::new(stream).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- sanitize_upload_filename: the client-name → safe-basename gate ---

    #[test]
    fn test_sanitize_upload_filename_keeps_a_plain_name() {
        assert_eq!(
            sanitize_upload_filename("report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            sanitize_upload_filename("  spaced.txt  ").as_deref(),
            Some("spaced.txt")
        );
    }

    #[test]
    fn test_sanitize_upload_filename_strips_any_directory_component() {
        // A traversal attempt collapses to its final path component.
        assert_eq!(
            sanitize_upload_filename("../../evil.sh").as_deref(),
            Some("evil.sh")
        );
        assert_eq!(
            sanitize_upload_filename("/etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_upload_filename("a/b/c/d.txt").as_deref(),
            Some("d.txt")
        );
    }

    #[test]
    fn test_sanitize_upload_filename_rejects_empty_and_dot_names() {
        assert_eq!(sanitize_upload_filename(""), None);
        assert_eq!(sanitize_upload_filename("   "), None);
        assert_eq!(sanitize_upload_filename("."), None);
        assert_eq!(sanitize_upload_filename(".."), None);
    }

    #[test]
    fn test_sanitize_upload_filename_rejects_control_characters() {
        assert_eq!(sanitize_upload_filename("bad\nname.txt"), None);
        assert_eq!(sanitize_upload_filename("tab\tname.txt"), None);
        assert_eq!(sanitize_upload_filename("nul\0name.txt"), None);
    }

    #[test]
    fn test_sanitize_upload_filename_keeps_a_leading_dot_file() {
        assert_eq!(
            sanitize_upload_filename(".gitignore").as_deref(),
            Some(".gitignore")
        );
    }

    // --- strip_partition_suffix: partition → parent block device ---

    #[test]
    fn test_strip_partition_suffix_sata_scsi_virtio() {
        assert_eq!(strip_partition_suffix("sda1"), "sda");
        assert_eq!(strip_partition_suffix("sdb15"), "sdb");
        assert_eq!(strip_partition_suffix("vda2"), "vda");
        // A whole disk with no partition number is unchanged.
        assert_eq!(strip_partition_suffix("sdc"), "sdc");
    }

    #[test]
    fn test_strip_partition_suffix_nvme_and_mmcblk_use_p_separator() {
        assert_eq!(strip_partition_suffix("nvme0n1p2"), "nvme0n1");
        assert_eq!(strip_partition_suffix("mmcblk0p1"), "mmcblk0");
        // The trailing digit after 'n'/no 'p' is part of the device name,
        // not a partition — must not be trimmed.
        assert_eq!(strip_partition_suffix("nvme0n1"), "nvme0n1");
        assert_eq!(strip_partition_suffix("mmcblk0"), "mmcblk0");
    }

    // --- kind_for_root: allowed-root path → sidebar `kind` key ---

    #[test]
    fn test_kind_for_root_classifies_the_well_known_system_roots() {
        assert_eq!(kind_for_root("/home"), "users");
        assert_eq!(kind_for_root("/srv"), "shares");
        assert_eq!(kind_for_root("/media"), "media");
        assert_eq!(kind_for_root("/mnt"), "media");
        assert_eq!(kind_for_root("/"), "system");
    }

    #[test]
    fn test_kind_for_root_flags_an_app_data_root_and_defaults_to_storage() {
        assert_eq!(kind_for_root("/opt/homevault/data"), "appdata");
        // A nested path under /mnt is not the /mnt root itself.
        assert_eq!(kind_for_root("/mnt/tank/media"), "storage");
        assert_eq!(kind_for_root("/data/pool1"), "storage");
    }

    // --- resolve_dest: CollisionStrategy applied to a concrete path ---

    #[test]
    fn test_resolve_dest_overwrite_always_writes_the_requested_path() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("f.txt");
        std::fs::write(&taken, b"x").unwrap();
        let p = taken.to_string_lossy().into_owned();
        match resolve_dest(&p, None) {
            ResolvedDest::Write(w) => assert_eq!(w, p),
            ResolvedDest::Skip => panic!("overwrite must not skip"),
        }
    }

    #[test]
    fn test_resolve_dest_skip_only_skips_when_the_target_exists() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("f.txt");
        std::fs::write(&taken, b"x").unwrap();
        let free = dir.path().join("g.txt");

        assert!(matches!(
            resolve_dest(&taken.to_string_lossy(), Some("skip")),
            ResolvedDest::Skip
        ));
        assert!(matches!(
            resolve_dest(&free.to_string_lossy(), Some("skip")),
            ResolvedDest::Write(_)
        ));
    }

    #[test]
    fn test_resolve_dest_keep_both_rewrites_a_colliding_name() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("f.txt");
        std::fs::write(&taken, b"x").unwrap();
        match resolve_dest(&taken.to_string_lossy(), Some("keep_both")) {
            ResolvedDest::Write(w) => {
                assert_eq!(w, dir.path().join("f (1).txt").to_string_lossy());
            }
            ResolvedDest::Skip => panic!("keep_both never skips"),
        }
    }

    // --- parse_range: HTTP Range header → (start, end) / 416 ---

    fn headers_with_range(range: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("range", range.parse().unwrap());
        h
    }

    #[test]
    fn test_parse_range_absent_means_whole_file() {
        let h = HeaderMap::new();
        assert_eq!(parse_range(&h, 100).unwrap(), None);
    }

    #[test]
    fn test_parse_range_basic_and_open_ended() {
        let (s, e) = parse_range(&headers_with_range("bytes=10-19"), 100)
            .unwrap()
            .unwrap();
        assert_eq!((s, e), (10, 19));
        let (s, e) = parse_range(&headers_with_range("bytes=90-"), 100)
            .unwrap()
            .unwrap();
        assert_eq!((s, e), (90, 99));
    }

    #[test]
    fn test_parse_range_end_clamped_to_file_size() {
        let (s, e) = parse_range(&headers_with_range("bytes=50-999"), 100)
            .unwrap()
            .unwrap();
        assert_eq!((s, e), (50, 99));
    }

    #[test]
    fn test_parse_range_suffix_form_is_last_n_bytes() {
        let (s, e) = parse_range(&headers_with_range("bytes=-20"), 100)
            .unwrap()
            .unwrap();
        assert_eq!((s, e), (80, 99));
    }

    #[test]
    fn test_parse_range_unsatisfiable_or_malformed_is_err() {
        assert!(
            parse_range(&headers_with_range("bytes=100-"), 100).is_err(),
            "start == total"
        );
        assert!(
            parse_range(&headers_with_range("bytes=20-10"), 100).is_err(),
            "end < start"
        );
        assert!(
            parse_range(&headers_with_range("bytes=0-1,3-4"), 100).is_err(),
            "multi-range"
        );
        assert!(
            parse_range(&headers_with_range("items=0-1"), 100).is_err(),
            "wrong unit"
        );
        assert!(
            parse_range(&headers_with_range("bytes=abc"), 100).is_err(),
            "garbage"
        );
        assert!(
            parse_range(&headers_with_range("bytes=0-"), 0).is_err(),
            "empty file"
        );
    }
}
