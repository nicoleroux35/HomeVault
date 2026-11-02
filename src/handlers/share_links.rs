/* =============================================================================
 * File          : share_links.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   "Share by link" for the Explorer: mint a revocable, optionally expiring
 *   download link to a file the caller can access, list one's links, revoke
 *   a link (owner or admin), and serve the public token-authenticated
 *   download. The token is the secret — high-entropy, never logged; links
 *   default to 30 days and can be restricted to signed-in users, in which
 *   case the session token must travel in a header, never in a URL. A link
 *   can also carry a password (Argon2, never returned) and a download cap
 *   (`max_downloads = 1` is the one-time link).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - bytes
 *   - futures-util
 *   - parking_lot (>= 0.12)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   GET/POST/DELETE /api/explorer/shares and public GET /share/{token} in
 *   src/web/mod.rs; see the module doc for the full security posture.
 * =============================================================================
 */

//! Explorer "share by link" handlers.
//!
//! Lets a user mint a revocable, optionally expiring download link to a file
//! they can access, then share that link (or its QR code) outside HomeVault:
//!
//!   GET    /api/explorer/shares            list the caller's links
//!   POST   /api/explorer/shares            create a link for a file
//!   DELETE /api/explorer/shares/{token}    revoke a link (owner or admin)
//!   POST   /share/{token}/unlock           exchange the password for a cookie
//!   GET    /share/{token}                  public download (no session)
//!
//! Security / privacy posture (matches CLAUDE.md's compliance baseline):
//! - The **token is the secret** — high-entropy, URL-safe, never logged.
//! - Links default to `anyone` + a 30-day expiry; a link can be restricted to
//!   `signed_in` (download requires a valid session via the `x-session-token`
//!   header) or set to never expire. Revoking a link cuts access instantly.
//! - A password never enters a URL: the public page posts it once to
//!   `/share/{token}/unlock` and receives a short-lived, path-scoped, HttpOnly
//!   unlock cookie, so the download itself is an ordinary navigation (ranges,
//!   resume, no multi-gigabyte blob in the browser's memory). API clients can
//!   keep sending the password in the `x-share-password` header instead.
//! - A `max_downloads` cap reserves a slot per in-flight transfer and counts
//!   it only when the last byte was written, so a failed or abandoned download
//!   never consumes a one-time link.
//! - The session token is never accepted on the public download URL — a
//!   `signed_in` link only works from a client that can send the header, so
//!   the token never travels in a query string or access log.
//! - Creation/revocation are recorded in the plain-English event log.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
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

/// Lifetime of an unlock cookie minted by `POST /share/{token}/unlock`.
///
/// Long enough for a browser to start and finish a large download (a video
/// seek re-requests with `Range`), short enough that a shared machine does not
/// stay unlocked. The cookie is scoped to the one link's path and `HttpOnly`,
/// so a script can never read it, and it identifies the *unlock* rather than
/// the password — which therefore never reaches a URL, a log or the browser's
/// history.
const SHARE_UNLOCK_TTL: Duration = Duration::from_secs(15 * 60);
const SHARE_UNLOCK_COOKIE: &str = "hv_share_unlock";

/// Wrong-password budget per link inside [`SHARE_PASSWORD_WINDOW`].
///
/// Argon2 verification is deliberately expensive (tens of milliseconds and a
/// sizable memory arena), so an unthrottled password check on a public route
/// is a CPU/memory amplifier. This caps the cost an attacker can force while
/// leaving a household plenty of room for typos.
const SHARE_PASSWORD_MAX_FAILURES: u32 = 8;
const SHARE_PASSWORD_WINDOW: Duration = Duration::from_secs(5 * 60);

struct Unlock {
    nonce: String,
    expires_at: Instant,
}

struct PasswordFailures {
    count: u32,
    since: Instant,
}

/// Password unlocks and wrong-password accounting for public links.
///
/// Deliberately in memory: an unlock is worth minutes, never survives a
/// restart, and a fresh password recreates it. Both maps are swept lazily on
/// every access, so a long-running server cannot grow them without bound.
/// `now` is a parameter (not `Instant::now()` inside) so the window and the
/// expiry are testable without sleeping.
#[derive(Default)]
struct ShareAuthState {
    unlocks: HashMap<String, Unlock>,
    failures: HashMap<String, PasswordFailures>,
}

impl ShareAuthState {
    fn prune(&mut self, now: Instant) {
        self.unlocks.retain(|_, u| u.expires_at > now);
        self.failures
            .retain(|_, f| now.duration_since(f.since) < SHARE_PASSWORD_WINDOW);
    }

    fn unlocked(&mut self, token: &str, nonce: &str, now: Instant) -> bool {
        self.prune(now);
        self.unlocks
            .get(token)
            .is_some_and(|u| u.nonce == nonce && u.expires_at > now)
    }

    fn grant(&mut self, token: &str, nonce: &str, now: Instant) {
        self.prune(now);
        self.unlocks.insert(
            token.to_string(),
            Unlock {
                nonce: nonce.to_string(),
                expires_at: now + SHARE_UNLOCK_TTL,
            },
        );
    }

    fn revoke(&mut self, token: &str) {
        self.unlocks.remove(token);
        self.failures.remove(token);
    }

    fn may_try_password(&mut self, token: &str, now: Instant) -> bool {
        self.prune(now);
        self.failures
            .get(token)
            .is_none_or(|f| f.count < SHARE_PASSWORD_MAX_FAILURES)
    }

    fn note_password_failure(&mut self, token: &str, now: Instant) {
        self.prune(now);
        let entry = self
            .failures
            .entry(token.to_string())
            .or_insert(PasswordFailures {
                count: 0,
                since: now,
            });
        entry.count += 1;
    }

    fn clear_password_failures(&mut self, token: &str) {
        self.failures.remove(token);
    }
}

static SHARE_AUTH: LazyLock<Mutex<ShareAuthState>> =
    LazyLock::new(|| Mutex::new(ShareAuthState::default()));

/// A share token is minted from `[A-Za-z0-9]`, so anything else in the path is
/// not a link this server created. Checking it before the token is echoed
/// anywhere (a cookie `Path`, a header) keeps a crafted path from injecting
/// into a response header.
fn token_is_well_formed(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A browser address-bar navigation asks for HTML; a download manager, an API
/// client and `fetch()` (whose default is `Accept: */*`) do not. Only the first
/// kind can meaningfully be handed the public page instead of a JSON refusal,
/// which is what lets `/share/<token>` stay a one-click download *and* explain
/// itself when the visitor has to type a password first.
fn wants_html(method: &Method, headers: &HeaderMap) -> bool {
    *method == Method::GET
        && headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html"))
}

/// The `hv_session` cookie a login sets, so a browser navigating to a
/// `signed_in` link is recognised without an `x-session-token` header (a
/// navigation cannot carry one).
fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    crate::app_proxy::cookie::session_from_cookie_header(raw).map(str::to_string)
}

/// The unlock nonce from the request's cookies, if any.
fn share_unlock_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| {
            (name == SHARE_UNLOCK_COOKIE && !value.is_empty()).then(|| value.to_string())
        })
}

fn unlock_cookie(token: &str, nonce: &str, over_tls: bool) -> String {
    let secure = if over_tls { "; Secure" } else { "" };
    format!(
        "{SHARE_UNLOCK_COOKIE}={nonce}; Path=/share/{token}; HttpOnly; SameSite=Lax{secure}; Max-Age={}",
        SHARE_UNLOCK_TTL.as_secs()
    )
}

/// A `401` that says *why* the caller was refused (`password` vs `session`) so
/// the public page can show the right next step. It reveals nothing beyond what
/// possession of the token already proves.
fn share_auth_refusal(message: &str, reason: &str) -> Response {
    let mut response = ResponseHelper::json_unauthorized(message.to_string());
    if let Ok(value) = HeaderValue::from_str(reason) {
        response.headers_mut().insert("x-hv-share-auth", value);
    }
    response
}

fn share_not_found() -> Response {
    ResponseHelper::json_error("Share link not found".to_string())
}

/// The recipient's page, for a browser navigation that cannot be answered with
/// the file itself. It then probes the link and shows the right next step.
async fn share_page(nonce: Option<axum::Extension<crate::middleware::CspNonce>>) -> Response {
    crate::handlers::spa::serve_spa(nonce).await
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

struct CompletionShareStream<S> {
    inner: S,
    token: String,
    repo: ShareLinkRepository,
    expected: u64,
    sent: u64,
    finished: bool,
}

impl<S> Drop for CompletionShareStream<S> {
    fn drop(&mut self) {
        // The transfer ends here far more often than in `poll_next`, in both
        // directions:
        // - a client that disconnects mid-stream is never polled again, and
        // - **hyper stops polling a body whose `Content-Length` it has fully
        //   written**, so a *successful* download usually ends with this drop
        //   and no `Poll::Ready(None)` at all. Completion therefore follows the
        //   byte count (`sent == expected`), never the poll that happened to
        //   come last — otherwise every real download would be recorded as
        //   interrupted and a one-time link would never be spent.
        let complete = self.sent == self.expected;
        self.finish(complete);
    }
}

impl<S> Stream for CompletionShareStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.sent += chunk.len() as u64;
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finish(false);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                // Same rule as `Drop`: the bytes actually written decide.
                let complete = self.sent == self.expected;
                self.finish(complete);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> CompletionShareStream<S> {
    fn finish(&mut self, completed: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        // `Drop` can run outside a runtime (a caller dropping the response), and
        // `tokio::spawn` would panic there. Losing the release is recoverable —
        // the slot is released on the next server start — a panic is not.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let token = self.token.clone();
        let repo = self.repo.clone();
        tokio::spawn(async move {
            let _ = repo.finish_download(&token, completed).await;
        });
    }
}

#[derive(Debug, Deserialize)]
pub struct ShareLinkCreateRequest {
    pub path: String,
    /// `anyone` (public) or `signed_in` (any authenticated HomeVault session).
    pub mode: Option<String>,
    /// Lifetime in days; `None`/omitted → default; `0` → never expires.
    pub expires_days: Option<i64>,
    pub password: Option<String>,
    pub max_downloads: Option<i64>,
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
    /// True when the link carries a password (the hash itself never leaves
    /// the server).
    pub protected: bool,
    /// True once `max_downloads` has been consumed — the link is now dead.
    pub exhausted: bool,
}

#[derive(Debug, Deserialize)]
pub struct ShareUnlockRequest {
    pub password: String,
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
    let protected = link.password_hash.is_some();
    let exhausted = is_exhausted(&link);
    ShareLinkView {
        link,
        share_path,
        expired,
        protected,
        exhausted,
    }
}

/// A capped link with no slots left. `download_count` only rises when a
/// transfer completed, so a failed or abandoned download never spends a slot.
fn is_exhausted(link: &ShareLink) -> bool {
    link.max_downloads
        .is_some_and(|max| link.download_count >= max)
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
    if !file_path.is_file() {
        return ResponseHelper::json_error("Only regular files can be shared by link".to_string())
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
    if req
        .password
        .as_ref()
        .is_some_and(|p| p.len() < 8 || p.len() > 256)
    {
        return ResponseHelper::json_error(
            "Share password must be between 8 and 256 characters".to_string(),
        )
        .into_response();
    }
    if req
        .max_downloads
        .is_some_and(|n| !(1..=100_000).contains(&n))
    {
        return ResponseHelper::json_error(
            "max_downloads must be between 1 and 100000".to_string(),
        )
        .into_response();
    }
    let password_hash = match req
        .password
        .as_deref()
        .map(|p| state.auth.hash_password(p))
        .transpose()
    {
        Ok(hash) => hash,
        Err(_) => {
            return ResponseHelper::json_error("Could not protect share link".to_string())
                .into_response()
        }
    };
    match repo
        .create(
            &session.username,
            &token,
            &path,
            &mode,
            expires_at.as_deref(),
            password_hash.as_deref(),
            req.max_downloads,
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
            // A revoked link must also drop any unlock in flight, so a cookie
            // minted a minute ago cannot outlive the revoke.
            SHARE_AUTH.lock().revoke(&token);
            crate::events::log_event(format!(
                "User '{}' revoked share link for file '{}' (created by '{}')",
                username, link.path, link.username
            ));
            ResponseHelper::json_success(serde_json::json!({ "revoked": true }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke share link: {}", e)),
    }
}

/// POST /share/{token}/unlock — exchange the link's password for a
/// short-lived, path-scoped unlock cookie.
///
/// The password is verified exactly once, here; every later request (the
/// download, a `Range` seek, a resume) carries the cookie instead. That keeps
/// the password out of every URL and lets the browser download the file the
/// way it downloads anything else — streamed to disk with ranges, rather than
/// fetched into a JavaScript blob first.
///
/// Public by design (the token is the only credential) but deliberately
/// indistinguishable for a nonexistent, expired or password-less link: all
/// answer the same refusal, so the endpoint cannot be used as a token oracle.
pub async fn handle_share_link_unlock(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    over_tls: Option<axum::Extension<crate::middleware::ArrivedOverTls>>,
    axum::extract::Json(req): axum::extract::Json<ShareUnlockRequest>,
) -> Response {
    if !token_is_well_formed(&token) {
        return share_not_found();
    }

    let repo = ShareLinkRepository::new(state.db.pool());
    let Some(link) = repo.find_by_token(&token).await.ok().flatten() else {
        return share_not_found();
    };
    if is_expired(&link) {
        return share_not_found();
    }
    let Some(hash) = link.password_hash.as_deref() else {
        return ResponseHelper::json_error("This share link is not password protected".to_string());
    };

    let now = Instant::now();
    if !SHARE_AUTH.lock().may_try_password(&token, now) {
        return ResponseHelper::json_error_with_status(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many incorrect password attempts, try again in a few minutes".to_string(),
        );
    }
    // Bound the input before Argon2 sees it: hashing a megabyte from a public
    // route is free work for an attacker.
    if req.password.is_empty() || req.password.len() > 256 {
        SHARE_AUTH.lock().note_password_failure(&token, now);
        return share_auth_refusal("The share password is incorrect", "password");
    }
    if !state.auth.verify_password(&req.password, hash) {
        SHARE_AUTH.lock().note_password_failure(&token, now);
        return share_auth_refusal("The share password is incorrect", "password");
    }

    SHARE_AUTH.lock().clear_password_failures(&token);
    let nonce = generate_share_token();
    SHARE_AUTH.lock().grant(&token, &nonce, now);
    let cookie = unlock_cookie(&token, &nonce, over_tls.is_some());
    let mut response = ResponseHelper::json_success(serde_json::json!({ "unlocked": true }));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// GET /share/{token} — the public download. No session required for
/// `anyone` links; `signed_in` links need the `x-session-token` header
/// (never a query-string token — that would leak into logs).
pub async fn handle_share_link_download(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    method: Method,
    headers: HeaderMap,
    nonce: Option<axum::Extension<crate::middleware::CspNonce>>,
) -> Response {
    // A human navigating to a link gets the public page whenever the answer is
    // not the file itself — a password form, "this link is unavailable", or an
    // explanation for a signed-in-only link. A JSON error in the address bar
    // helps nobody, and the page's own `fetch` probes send `Accept: */*`, so
    // they still receive the status and the `x-hv-share-auth` reason.
    let page = wants_html(&method, &headers);
    if !token_is_well_formed(&token) {
        if page {
            return share_page(nonce).await;
        }
        return share_not_found();
    }

    let repo = ShareLinkRepository::new(state.db.pool());
    let Some(link) = repo.find_by_token(&token).await.ok().flatten() else {
        // Deliberately indistinguishable from an expired link: don't confirm
        // whether the token ever existed.
        if page {
            return share_page(nonce).await;
        }
        return share_not_found();
    };

    if is_expired(&link) {
        if page {
            return share_page(nonce).await;
        }
        return share_not_found();
    }

    // Either an unlock cookie minted by `/share/{token}/unlock` (the browser
    // path) or the password itself in a header (the API path). The password is
    // never read from the query string: that would put it in the access log.
    if let Some(hash) = &link.password_hash {
        let unlocked = share_unlock_cookie(&headers)
            .is_some_and(|nonce| SHARE_AUTH.lock().unlocked(&token, &nonce, Instant::now()));
        if !unlocked {
            let supplied = headers
                .get("x-share-password")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            // An empty password is a probe (the public page checks whether a
            // password is needed at all), not a guess: it costs no budget.
            if supplied.is_empty() {
                if page {
                    return share_page(nonce).await;
                }
                return share_auth_refusal("A valid share password is required", "password");
            }
            let now = Instant::now();
            if !SHARE_AUTH.lock().may_try_password(&token, now) {
                return ResponseHelper::json_error_with_status(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many incorrect password attempts, try again in a few minutes".to_string(),
                );
            }
            if supplied.len() > 256 || !state.auth.verify_password(supplied, hash) {
                SHARE_AUTH.lock().note_password_failure(&token, now);
                if page {
                    return share_page(nonce).await;
                }
                return share_auth_refusal("The share password is incorrect", "password");
            }
            SHARE_AUTH.lock().clear_password_failures(&token);
        }
    }

    if link.mode == "signed_in" {
        // A header (API client) or the login cookie a browser already holds:
        // a plain navigation cannot send a header, so without the cookie a
        // signed-in owner could never open their own link in a tab.
        let from_header = headers
            .get("x-session-token")
            .and_then(|h| h.to_str().ok())
            .filter(|token| !token.is_empty());
        let mut authorised = false;
        for candidate in [from_header.map(str::to_string), session_cookie(&headers)]
            .into_iter()
            .flatten()
        {
            if state.auth.get_session(&candidate).await.is_ok() {
                authorised = true;
                break;
            }
        }
        if !authorised {
            if page {
                return share_page(nonce).await;
            }
            return share_auth_refusal("This share requires a HomeVault session", "session");
        }
    }

    let file_path = std::path::Path::new(&link.path);
    if !file_path.exists() || !file_path.is_file() {
        if page {
            return share_page(nonce).await;
        }
        return share_not_found();
    }

    // The cap is checked before reserving so that even a `HEAD` probe reports
    // the truth: a spent link is gone, not "downloadable until you try".
    if is_exhausted(&link) {
        if page {
            return share_page(nonce).await;
        }
        return ResponseHelper::json_error(
            "This share link has reached its download limit".to_string(),
        );
    }

    let is_head = method == Method::HEAD;
    if !is_head && !repo.reserve_download(&token).await.unwrap_or(false) {
        return ResponseHelper::json_error(
            "This share link has reached its download limit".to_string(),
        );
    }
    // Every return between the reservation above and the response body must
    // release the slot; only the streaming body releases itself (on completion,
    // on a read error and on a client disconnect). A `HEAD` reserves nothing.
    let reserved = !is_head;
    macro_rules! release_slot {
        () => {
            if reserved {
                let _ = repo.finish_download(&token, false).await;
            }
        };
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
            release_slot!();
            return share_not_found();
        }
    };
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(_) => {
            release_slot!();
            return share_not_found();
        }
    };
    let total = metadata.len();

    // Range negotiation (resumable downloads / video seeking on public links).
    let range = match crate::handlers::explorer::parse_range(&headers, total) {
        Ok(r) => r,
        Err(()) => {
            release_slot!();
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
    //
    // A closed semaphore used to `unwrap()` into a panic — a 500 with no
    // message in the request log, and in a debug build a full task abort
    // (ROADMAP 10.7). It cannot happen with the static above (nothing calls
    // `close`), which is exactly why it went unnoticed: an `unwrap` on a
    // resource whose lifecycle lives elsewhere is a latent panic. `503` says
    // what happened and keeps the public endpoint answering.
    let permit = match SHARE_SEMAPHORE.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            tracing::error!(
                "Share streams: the concurrency semaphore is closed; refusing a public stream"
            );
            release_slot!();
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "The share streaming pool is unavailable, try again shortly",
            )
                .into_response();
        }
    };
    let body = if is_head || total == 0 {
        if reserved {
            let _ = repo.finish_download(&token, true).await;
        }
        Body::empty()
    } else {
        use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
        let start = range.map(|(s, _)| s).unwrap_or(0);
        if start > 0 {
            if file.seek(SeekFrom::Start(start)).await.is_err() {
                let _ = repo.finish_download(&token, false).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to seek file").into_response();
            }
        }
        let limited = file.take(send_len);
        let stream = CompletionShareStream {
            inner: RateLimitedShareStream {
                inner: tokio_util::io::ReaderStream::new(limited),
                token: token.clone(),
                _permit: permit,
            },
            token: token.clone(),
            repo: repo.clone(),
            expected: send_len,
            sent: 0,
            finished: false,
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
    // A public link is a grant that can expire, be revoked or run out of
    // downloads *after* this response was produced: nothing may keep a copy.
    // (The service worker refuses `/share/` for the same reason.)
    .header(header::CACHE_CONTROL, "no-store")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// ROADMAP 10.7: a closed semaphore must produce a mapped error, not a
    /// panic. The production semaphore is never closed, so the test drives a
    /// clone of the same `Arc` — the shape the handler relies on.
    #[tokio::test]
    async fn a_closed_semaphore_is_a_service_unavailable_not_a_panic() {
        let sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SHARE_STREAMS));
        sem.close();
        let acquired = sem.clone().acquire_owned().await;
        assert!(
            acquired.is_err(),
            "a closed semaphore must refuse the permit"
        );
        // …and the handler maps that to a response rather than unwrapping: the
        // source must not contain the old `unwrap()` on this path.
        let src = include_str!("share_links.rs");
        // Built by `concat!` so the needle does not occur literally in this
        // file: written as one string literal, the assertion matched its own
        // argument and could never pass.
        let forbidden = concat!("acquire_owned().await", ".unwrap()");
        assert!(
            !src.contains(forbidden),
            "the public stream path must not unwrap the semaphore (ROADMAP 10.7)"
        );
        let answer = concat!("StatusCode::", "SERVICE_UNAVAILABLE");
        assert!(
            src.contains(answer),
            "a closed pool must answer 503, not panic"
        );
    }

    /// The concurrency ceiling still exists — the fix is the error mapping, not
    /// the removal of the limit.
    #[tokio::test]
    async fn the_stream_ceiling_still_bounds_concurrency() {
        let sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SHARE_STREAMS));
        let mut held = Vec::new();
        for _ in 0..MAX_CONCURRENT_SHARE_STREAMS {
            held.push(sem.clone().acquire_owned().await.expect("permit"));
        }
        assert!(
            sem.clone().try_acquire_owned().is_err(),
            "the {MAX_CONCURRENT_SHARE_STREAMS}-stream ceiling must hold"
        );
        drop(held);
        assert!(sem.try_acquire_owned().is_ok());
    }

    // -----------------------------------------------------------------------
    // Password unlock + download-cap arithmetic
    // -----------------------------------------------------------------------

    fn link(max_downloads: Option<i64>, download_count: i64) -> ShareLink {
        ShareLink {
            id: 1,
            token: "tok".to_string(),
            username: "u".to_string(),
            path: "/f.bin".to_string(),
            mode: "anyone".to_string(),
            expires_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            password_hash: None,
            max_downloads,
            download_count,
            active_downloads: 0,
        }
    }

    #[test]
    fn an_uncapped_link_is_never_exhausted() {
        assert!(!is_exhausted(&link(None, 9_999)));
    }

    #[test]
    fn the_nth_download_is_the_last_one() {
        // A cap of 2 is spent once two downloads *completed*.
        assert!(!is_exhausted(&link(Some(2), 0)));
        assert!(!is_exhausted(&link(Some(2), 1)));
        assert!(is_exhausted(&link(Some(2), 2)));
    }

    #[test]
    fn a_one_time_link_is_dead_after_one_completed_download() {
        // `max_downloads = 1` is what the UI calls a one-time link.
        assert!(!is_exhausted(&link(Some(1), 0)));
        assert!(is_exhausted(&link(Some(1), 1)));
    }

    #[test]
    fn only_minted_tokens_are_welformed() {
        assert!(token_is_well_formed(&generate_share_token()));
        assert!(token_is_well_formed("aZ09-_"));
        assert!(!token_is_well_formed(""));
        // A path segment is echoed into a cookie `Path`, so anything that could
        // break a header out of it must be refused before that happens.
        assert!(!token_is_well_formed("abc;Path=/"));
        assert!(!token_is_well_formed("a b"));
        assert!(!token_is_well_formed(&"A".repeat(129)));
    }

    #[test]
    fn the_unlock_cookie_is_scoped_http_only_and_short_lived() {
        let cookie = unlock_cookie("tok123", "nonce", true);
        assert!(cookie.starts_with("hv_share_unlock=nonce;"));
        assert!(cookie.contains("Path=/share/tok123"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains(&format!("Max-Age={}", SHARE_UNLOCK_TTL.as_secs())));
        // Plain HTTP must not set a cookie the browser would then reject.
        assert!(!unlock_cookie("tok123", "nonce", false).contains("Secure"));
    }

    #[test]
    fn the_unlock_cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; hv_share_unlock=abc123; lang=fr"),
        );
        assert_eq!(share_unlock_cookie(&headers).as_deref(), Some("abc123"));
        let empty = HeaderMap::new();
        assert_eq!(share_unlock_cookie(&empty), None);
        let mut blank = HeaderMap::new();
        blank.insert(header::COOKIE, HeaderValue::from_static("hv_share_unlock="));
        assert_eq!(share_unlock_cookie(&blank), None);
    }

    #[test]
    fn an_unlock_expires_and_is_bound_to_its_link() {
        let mut state = ShareAuthState::default();
        let now = Instant::now();
        state.grant("tok", "nonce", now);
        assert!(state.unlocked("tok", "nonce", now));
        assert!(state.unlocked(
            "tok",
            "nonce",
            now + SHARE_UNLOCK_TTL - Duration::from_secs(1)
        ));
        // Expired…
        assert!(!state.unlocked("tok", "nonce", now + SHARE_UNLOCK_TTL));
        // …another link's token never accepts it…
        assert!(!state.unlocked("other", "nonce", now));
        // …and neither does another nonce on the same link.
        state.grant("tok", "nonce", now);
        assert!(!state.unlocked("tok", "forged", now));
    }

    #[test]
    fn revoking_a_link_drops_its_unlock_and_its_failures() {
        let mut state = ShareAuthState::default();
        let now = Instant::now();
        state.grant("tok", "nonce", now);
        state.note_password_failure("tok", now);
        state.revoke("tok");
        assert!(!state.unlocked("tok", "nonce", now));
        assert!(state.may_try_password("tok", now));
    }

    #[test]
    fn the_password_budget_is_finite_and_refills_after_the_window() {
        let mut state = ShareAuthState::default();
        let now = Instant::now();
        for attempt in 1..=SHARE_PASSWORD_MAX_FAILURES {
            assert!(
                state.may_try_password("tok", now),
                "attempt {attempt} allowed"
            );
            state.note_password_failure("tok", now);
        }
        // The budget is spent: Argon2 must not be reachable any more.
        assert!(!state.may_try_password("tok", now));
        assert!(!state.may_try_password("tok", now + SHARE_PASSWORD_WINDOW / 2));
        // …until the window has passed, so a household is never locked out by
        // yesterday's typos (and a restart clears it anyway).
        assert!(state.may_try_password("tok", now + SHARE_PASSWORD_WINDOW));
        // A correct password clears the count immediately.
        state.note_password_failure("tok", now);
        state.clear_password_failures("tok");
        assert!(state.may_try_password("tok", now));
        // Failures on one link never punish another.
        assert!(state.may_try_password("other", now));
    }
}
