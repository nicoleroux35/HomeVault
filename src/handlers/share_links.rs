//! Explorer "share by link" handlers.
//!
//! Lets a user mint a revocable, optionally expiring download link to a file
//! they can access, then share that link (or its QR code) outside HomeVault:
//!
//!   GET    /api/explorer/shares            list the caller's links
//!   POST   /api/explorer/shares            create a link for a file
//!   DELETE /api/explorer/shares/{token}    revoke a link (owner or admin)
//!   GET    /share/{token}                  public download (no session)
//!
//! Security / privacy posture (matches CLAUDE.md's compliance baseline):
//! - The **token is the secret** — high-entropy, URL-safe, never logged.
//! - Links default to `anyone` + a 30-day expiry; a link can be restricted to
//!   `signed_in` (download requires a valid session via the `x-session-token`
//!   header) or set to never expire. Revoking a link cuts access instantly.
//! - The session token is never accepted on the public download URL — a
//!   `signed_in` link only works from a client that can send the header, so
//!   the token never travels in a query string or access log.
//! - Creation/revocation are recorded in the plain-English event log.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::Stream;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::handlers::AppState;
use crate::storage::{ShareLink, ShareLinkRepository};
use crate::utils::ResponseHelper;

const ALLOWED_MODES: [&str; 2] = ["anyone", "signed_in"];
/// Default lifetime of a share link (days) when the caller doesn't specify one.
const DEFAULT_EXPIRY_DAYS: i64 = 30;

/// Per-token bandwidth budget for the *public* `/share/{token}` stream.
///
/// A public link is reachable by anyone who has the token, so it can be
/// hammered to drain the server's bandwidth. Each token gets a leaky bucket:
/// a burst cap and a sustained refill rate; once exhausted the stream is cut
/// (a truncated download, not a DoS vector). Authenticated Explorer
/// downloads are *not* rate-limited — they're covered by transfer quotas.
const SHARE_RATE_CAPACITY: f64 = 64.0 * 1024.0 * 1024.0; // 64 MiB burst
const SHARE_RATE_REFILL: f64 = 8.0 * 1024.0 * 1024.0; // 8 MiB/s sustained
/// Max simultaneous public share streams (boundary on aggregate bandwidth).
const MAX_CONCURRENT_SHARE_STREAMS: usize = 4;

struct ShareBucket {
    tokens: f64,
    last: Instant,
}

static SHARE_BUCKETS: LazyLock<Mutex<HashMap<String, ShareBucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SHARE_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SHARE_STREAMS)));

/// Tries to draw `bytes` out of the token's bucket. Returns `false` once the
/// bucket is empty (the stream is then terminated).
fn share_acquire(token: &str, bytes: u64) -> bool {
    let now = Instant::now();
    let mut map = SHARE_BUCKETS.lock();
    map.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(300));
    let bucket = map.entry(token.to_string()).or_insert(ShareBucket {
        tokens: SHARE_RATE_CAPACITY,
        last: now,
    });
    let elapsed = now.duration_since(bucket.last).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * SHARE_RATE_REFILL).min(SHARE_RATE_CAPACITY);
    bucket.last = now;
    if bucket.tokens >= bytes as f64 {
        bucket.tokens -= bytes as f64;
        true
    } else {
        false
    }
}

/// Stream wrapper that applies the per-token bandwidth bucket and holds the
/// global concurrency permit for the life of the response body.
struct RateLimitedShareStream<S> {
    inner: S,
    token: String,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<S> Stream for RateLimitedShareStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if share_acquire(&self.token, chunk.len() as u64) {
                    Poll::Ready(Some(Ok(chunk)))
                } else {
                    // Bucket exhausted — stop streaming (hard cap).
                    Poll::Ready(None)
                }
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ShareLinkCreateRequest {
    pub path: String,
    /// `anyone` (public) or `signed_in` (any authenticated HomeVault session).
    pub mode: Option<String>,
    /// Lifetime in days; `None`/omitted → default; `0` → never expires.
    pub expires_days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ShareLinkView {
    #[serde(flatten)]
    pub link: ShareLink,
    /// Relative public path, e.g. `/share/<token>` — the client turns this
    /// into an absolute URL with its own origin.
    pub share_path: String,
    /// True when `expires_at` is set and already in the past.
    pub expired: bool,
}

/// Pure helper so the repo's unit tests can pin the token shape.
///
/// Uses rejection sampling so every character of the 62-char alphabet is
/// exactly equally likely — the naive `byte % 62` biases a few characters by
/// `256 % 62 = 8` in 256.
pub fn generate_share_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    const CS_LEN: u32 = 62;
    // Largest multiple of 62 below 2^32; values at/above it are rejected so
    // every character of the alphabet is exactly equally likely.
    const LIMIT: u32 = u32::MAX - (u32::MAX % CS_LEN);
    let mut rng = rand::rng();
    let mut out = String::with_capacity(48);
    while out.len() < 48 {
        let mut buf = [0u8; 4];
        rng.fill_bytes(&mut buf);
        let v = u32::from_le_bytes(buf);
        if v < LIMIT {
            out.push(CHARSET[(v % CS_LEN) as usize] as char);
        }
    }
    out
}

/// Expiry timestamp for a share link. `days <= 0` → no expiry.
pub fn compute_expiry(days: i64) -> Option<String> {
    if days <= 0 {
        return None;
    }
    let expiry = chrono::Utc::now() + chrono::Duration::days(days);
    Some(expiry.to_rfc3339())
}

fn to_view(link: ShareLink) -> ShareLinkView {
    let expired = match &link.expires_at {
        Some(expiry) => chrono::DateTime::parse_from_rfc3339(expiry)
            .map(|t| t.timestamp() < chrono::Utc::now().timestamp())
            .unwrap_or(false),
        None => false,
    };
    let share_path = format!("/share/{}", link.token);
    ShareLinkView {
        link,
        share_path,
        expired,
    }
}

/// GET /api/explorer/shares — the caller's share links, newest first.
pub async fn handle_share_links_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Ok(username) = crate::handlers::admin_guard::require_auth(&state, &headers).await else {
        return ResponseHelper::json_unauthorized("Authentication required".to_string());
    };

    let repo = ShareLinkRepository::new(state.db.pool());
    match repo.list_by_user(&username).await {
        Ok(links) => {
            let views: Vec<ShareLinkView> = links.into_iter().map(to_view).collect();
            ResponseHelper::json_success(views)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list share links: {}", e)),
    }
}

/// POST /api/explorer/shares — create a share link for a file.
pub async fn handle_share_link_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<ShareLinkCreateRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let session = match state.auth.get_session(session_token).await {
        Ok(s) => s,
        Err(_) => {
            return ResponseHelper::json_unauthorized("Session invalid".to_string()).into_response()
        }
    };

    let path = req.path.trim().to_string();
    if path.is_empty() || path == "/" {
        return ResponseHelper::json_error("Invalid path".to_string()).into_response();
    }

    // Scope the link to what the caller can actually reach, and store the
    // canonical resolved path so the public download never re-resolves a
    // symlink that could be swapped after the link is minted.
    let Some(canonical) = validate_path_access(&state, &path, &session) else {
        tracing::warn!(
            "Share link creation blocked: path '{}' outside allowed roots for {}",
            path,
            session.username
        );
        return ResponseHelper::json_forbidden(
            "Access denied: path outside allowed directory".to_string(),
        )
        .into_response();
    };
    let canonical = std::fs::canonicalize(&canonical)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| canonical.to_string_lossy().into_owned());
    let path = canonical;

    let file_path = std::path::Path::new(&path);
    if !file_path.exists() {
        return ResponseHelper::json_error("File not found".to_string()).into_response();
    }
    // Only single files are shareable by link — a directory would need an
    // ad-hoc archive step the current transfer pipeline doesn't offer.
    if file_path.is_dir() {
        return ResponseHelper::json_error("Only files can be shared by link".to_string())
            .into_response();
    }

    let mode = req.mode.unwrap_or_else(|| "anyone".to_string());
    if !ALLOWED_MODES.contains(&mode.as_str()) {
        return ResponseHelper::json_error(format!(
            "Invalid share mode '{}' (expected anyone or signed_in)",
            mode
        ))
        .into_response();
    }
    let expires_days = req.expires_days.unwrap_or(DEFAULT_EXPIRY_DAYS);
    if !(0..=3650).contains(&expires_days) {
        return ResponseHelper::json_error("expires_days must be between 0 and 3650".to_string())
            .into_response();
    }

    let repo = ShareLinkRepository::new(state.db.pool());
    let token = generate_share_token();
    let expires_at = compute_expiry(expires_days);
    match repo
        .create(
            &session.username,
            &token,
            &path,
            &mode,
            expires_at.as_deref(),
        )
        .await
    {
        Ok(link) => {
            crate::events::log_event(format!(
                "User '{}' created a share link for file '{}' (mode {}, expires {})",
                session.username,
                path,
                mode,
                expires_at.as_deref().unwrap_or("never")
            ));
            ResponseHelper::json_success(to_view(link)).into_response()
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create share link: {}", e))
            .into_response(),
    }
}

/// DELETE /api/explorer/shares/{token} — revoke a link. Owner or admin only.
pub async fn handle_share_link_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> impl IntoResponse {
    let Ok(username) = crate::handlers::admin_guard::require_auth(&state, &headers).await else {
        return ResponseHelper::json_unauthorized("Authentication required".to_string());
    };

    let repo = ShareLinkRepository::new(state.db.pool());
    let Some(link) = repo.find_by_token(&token).await.ok().flatten() else {
        return ResponseHelper::json_error("Share link not found".to_string());
    };

    // Owners revoke their own links; admins may revoke any link.
    let is_admin = state.auth.is_admin(&username).await;
    if link.username != username && !is_admin {
        return ResponseHelper::json_forbidden(
            "You can only revoke your own share links".to_string(),
        );
    }

    match repo.delete(&token).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "User '{}' revoked share link for file '{}' (created by '{}')",
                username, link.path, link.username
            ));
            ResponseHelper::json_success(serde_json::json!({ "revoked": true }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke share link: {}", e)),
    }
}

/// GET /share/{token} — the public download. No session required for
/// `anyone` links; `signed_in` links need the `x-session-token` header
/// (never a query-string token — that would leak into logs).
pub async fn handle_share_link_download(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    if token.len() > 128 || token.is_empty() {
        return ResponseHelper::json_error("Share link not found".to_string()).into_response();
    }

    let repo = ShareLinkRepository::new(state.db.pool());
    let Some(link) = repo.find_by_token(&token).await.ok().flatten() else {
        // Deliberately indistinguishable from an expired link: don't confirm
        // whether the token ever existed.
        return ResponseHelper::json_error("Share link not found".to_string()).into_response();
    };

    if is_expired(&link) {
        return ResponseHelper::json_error("Share link not found".to_string()).into_response();
    }

    if link.mode == "signed_in" {
        let session_token = headers
            .get("x-session-token")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if session_token.is_empty() || state.auth.get_session(session_token).await.is_err() {
            return ResponseHelper::json_unauthorized(
                "This share requires a HomeVault session".to_string(),
            )
            .into_response();
        }
    }

    let file_path = std::path::Path::new(&link.path);
    if !file_path.exists() || !file_path.is_file() {
        return ResponseHelper::json_error("Share link not found".to_string()).into_response();
    }

    // Stream the file — mirrors handle_explorer_download's response shape
    // (same content-type map, attachment disposition, ranges, digest) but with
    // no session / transfer-quota gating: the link itself is the
    // authorisation. Public streams are instead bounded by the per-token
    // bandwidth bucket and a global concurrency cap.
    let file_path = std::path::Path::new(&link.path);
    let mut file = match tokio::fs::File::open(file_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("Failed to open shared file '{}': {}", link.path, e);
            return ResponseHelper::json_error("Share link not found".to_string()).into_response();
        }
    };
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(_) => {
            return ResponseHelper::json_error("Share link not found".to_string()).into_response()
        }
    };
    let total = metadata.len();

    // Range negotiation (resumable downloads / video seeking on public links).
    let range = match crate::handlers::explorer::parse_range(&headers, total) {
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

    let digest = if total <= crate::handlers::explorer::DIGEST_THRESHOLD {
        crate::handlers::explorer::file_sha256(file_path).await
    } else {
        None
    };

    let content_type = crate::handlers::explorer::content_type_for_path(file_path);
    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    // Bound concurrent public streams, then build the (range-limited,
    // rate-limited) body.
    let permit = SHARE_SEMAPHORE.clone().acquire_owned().await.unwrap();
    let body = if total == 0 {
        Body::empty()
    } else {
        use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
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
        let stream = RateLimitedShareStream {
            inner: tokio_util::io::ReaderStream::new(limited),
            token: token.clone(),
            _permit: permit,
        };
        Body::from_stream(stream)
    };

    crate::handlers::explorer::file_response_headers(
        status,
        content_type,
        filename,
        total,
        range,
        digest,
    )
    .body(body)
    .unwrap_or_else(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to create streaming response",
        )
            .into_response()
    })
}

fn is_expired(link: &ShareLink) -> bool {
    match &link.expires_at {
        Some(expiry) => chrono::DateTime::parse_from_rfc3339(expiry)
            .map(|t| t.timestamp() < chrono::Utc::now().timestamp())
            .unwrap_or(false),
        None => false,
    }
}

/// Same path gate as the Explorer handlers: admins bypass, others are
/// confined to `PathGuard`'s allowed roots. Returns the canonical validated
/// path, which callers must use for the actual work (never the raw input).
fn validate_path_access(
    state: &Arc<AppState>,
    path: &str,
    session: &crate::storage::models::Session,
) -> Option<std::path::PathBuf> {
    if session.is_admin {
        Some(std::path::PathBuf::from(path))
    } else {
        state.path_guard.validate(path)
    }
}
