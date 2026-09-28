//! Terminal handler backed by a native pseudo-terminal (PTY).
//!
//! Runs entirely in-process: no external binary, no reverse proxy hop. Each
//! admin username can open several independent shell tabs; every tab is its
//! own PTY session (spawned via `sudo -u <user> -i`), keyed by an opaque
//! client-supplied `tab_id`. Every browser tab/reconnect that presents the
//! same `tab_id` attaches to the same PTY, sharing its output and merging
//! its input — mirroring how attaching multiple clients to one terminal
//! session normally behaves. A user is capped at `MAX_TABS_PER_USER` live
//! sessions so a client can't exhaust host resources by opening tabs.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch, RwLock};

use crate::handlers::AppState;

/// Number of buffered output chunks per session before slow subscribers start
/// missing frames (they'll just see a gap, not a stall — the PTY reader thread
/// must never block on a stuck client).
const OUTPUT_CHANNEL_CAPACITY: usize = 512;

/// Recent-output buffer replayed to a client the moment it (re)attaches, so a
/// reconnecting tab sees the last bit of context instead of a blank screen.
const SCROLLBACK_CAPACITY: usize = 64 * 1024;

/// How long a one-time terminal connection ticket stays valid before it must
/// be re-issued (see `handle_terminal_ticket`).
const TICKET_TTL: Duration = Duration::from_secs(15);

/// Hard cap on concurrent shell tabs per user. Each tab is a real `sudo -u
/// <user> -i` login shell plus two OS threads, so this bound keeps a
/// misbehaving or malicious client from exhausting host PIDs/memory.
const MAX_TABS_PER_USER: usize = 8;

/// Validates a client-supplied tab id before it is used as a map key. Opaque
/// to the server, but constrained so it can't be huge or carry odd bytes.
fn is_valid_tab_id(tab_id: &str) -> bool {
    !tab_id.is_empty()
        && tab_id.len() <= 64
        && tab_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// How long a tab may sit with no browser attached before the reaper kills
/// its shell. Reopening the Terminal within this window (or from another
/// device / after a browser restart, using the persisted tab id) reattaches
/// to the same shell; past it, the tab starts fresh. Bounds orphaned shells
/// from a client that never comes back.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// One live shell session shared by every client attached to a given
/// `(user, tab)`.
struct PtySession {
    master: Arc<StdMutex<Box<dyn MasterPty + Send>>>,
    child: Arc<StdMutex<Box<dyn Child + Send + Sync>>>,
    output_tx: broadcast::Sender<Bytes>,
    input_tx: mpsc::UnboundedSender<Bytes>,
    scrollback: Arc<StdMutex<VecDeque<u8>>>,
    /// Flips to `true` once the reader thread observes the shell has exited,
    /// so any attached `handle_socket` can close the WebSocket instead of
    /// leaving it open forever with a dead shell behind it.
    ended_rx: watch::Receiver<bool>,
    /// Live count of attached WebSockets, and — while that is zero — when it
    /// last dropped to zero, so the reaper can retire an abandoned tab.
    attached: Arc<std::sync::atomic::AtomicUsize>,
    idle_since: Arc<StdMutex<Option<Instant>>>,
}

impl PtySession {
    fn is_alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    fn kill(&self) {
        let _ = self.child.lock().unwrap().kill();
    }

    /// A shell nobody has been attached to for longer than the idle timeout.
    fn is_idle_expired(&self) -> bool {
        if self.attached.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            return false;
        }
        matches!(*self.idle_since.lock().unwrap(), Some(t) if t.elapsed() >= SESSION_IDLE_TIMEOUT)
    }

    /// Called by `handle_socket` on attach / detach.
    fn mark_attached(&self) {
        self.attached.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *self.idle_since.lock().unwrap() = None;
    }

    fn mark_detached(&self) {
        if self.attached.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) == 1 {
            *self.idle_since.lock().unwrap() = Some(Instant::now());
        }
    }
}

/// Tracks every live PTY session, keyed by `(username, tab_id)`.
pub struct PtyManager {
    sessions: RwLock<HashMap<(String, String), Arc<PtySession>>>,
    /// ticket -> (username, tab_id, issued_at)
    tickets: RwLock<HashMap<String, (String, String, Instant)>>,
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            tickets: RwLock::new(HashMap::new()),
        }
    }

    /// Returns the existing session for a `(user, tab)`, or spawns a new one
    /// if none exists or the previous shell has exited. Spawning a brand-new
    /// tab is refused once the user already holds `MAX_TABS_PER_USER` live
    /// sessions.
    async fn get_or_create_session(
        &self,
        username: &str,
        tab_id: &str,
    ) -> Result<Arc<PtySession>, String> {
        let key = (username.to_string(), tab_id.to_string());
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(&key) {
                if session.is_alive() {
                    return Ok(session.clone());
                }
            }
        }

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get(&key) {
            if session.is_alive() {
                return Ok(session.clone());
            }
        }

        // Reap this user's dead and long-abandoned tabs, then enforce the
        // per-user cap. A tab whose shell exited, or that nobody has been
        // attached to past the idle timeout, doesn't count against the limit.
        sessions.retain(|(u, _), s| {
            if u != username {
                return true;
            }
            let keep = s.is_alive() && !s.is_idle_expired();
            if !keep {
                s.kill();
            }
            keep
        });
        let live_for_user = sessions.keys().filter(|(u, _)| u == username).count();
        if !sessions.contains_key(&key) && live_for_user >= MAX_TABS_PER_USER {
            return Err(format!(
                "Terminal tab limit reached ({} open); close one before opening another",
                MAX_TABS_PER_USER
            ));
        }

        let session = spawn_session(username)?;
        sessions.insert(key, session.clone());
        Ok(session)
    }

    /// Kills and forgets a single `(user, tab)` session — used when a user
    /// closes that tab in the UI.
    pub async fn close_tab(&self, username: &str, tab_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(&(username.to_string(), tab_id.to_string())) {
            session.kill();
            tracing::info!("Closed terminal tab {} for user {}", tab_id, username);
        }
    }

    /// Kills and forgets *every* tab belonging to a single user. Used when
    /// that user's account is deleted, so none of their shells keep running
    /// as orphans after the account is gone.
    pub async fn kill_session(&self, username: &str) {
        let mut sessions = self.sessions.write().await;
        let mut killed = 0;
        sessions.retain(|(u, _), session| {
            if u == username {
                session.kill();
                killed += 1;
                false
            } else {
                true
            }
        });
        // Also drop any not-yet-redeemed tickets for the gone account.
        self.tickets
            .write()
            .await
            .retain(|_, (u, _, _)| u != username);
        if killed > 0 {
            tracing::info!("Killed {} terminal tab(s) for deleted user {}", killed, username);
        }
    }

    /// Kills every live session. Intended for use on server shutdown.
    pub async fn cleanup_all(&self) {
        let mut sessions = self.sessions.write().await;
        for ((username, tab_id), session) in sessions.drain() {
            session.kill();
            tracing::info!("Cleaned up terminal tab {} for user {}", tab_id, username);
        }
    }

    /// The real working directory of the shell behind `(username, tab_id)`,
    /// resolved from `/proc/<pid>/cwd` so the UI can offer path completion
    /// and show a truthful prompt. `sudo -u <user> -i` may run the shell as a
    /// descendant of the child we spawned, so walk the process tree a few
    /// levels and use the deepest cwd we can read.
    pub async fn get_session_cwd(&self, username: &str, tab_id: &str) -> Option<std::path::PathBuf> {
        let key = (username.to_string(), tab_id.to_string());
        let session = self.sessions.read().await.get(&key)?.clone();
        let pid = session.child.lock().unwrap().process_id()?;
        let mut found = resolve_pid_cwd(pid);
        for descendant in descendant_pids(pid) {
            if let Some(cwd) = resolve_pid_cwd(descendant) {
                found = Some(cwd);
            }
        }
        found
    }

    /// Kills and forgets every session whose shell has exited or that has sat
    /// unattached past `SESSION_IDLE_TIMEOUT`. Cheap enough to run
    /// opportunistically (see `issue_ticket`) instead of on a timer.
    async fn reap_stale(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|(u, tab), s| {
            let keep = s.is_alive() && !s.is_idle_expired();
            if !keep {
                s.kill();
                tracing::info!("Reaped stale terminal tab {} for user {}", tab, u);
            }
            keep
        });
    }

    /// Issues a short-lived, single-use ticket for `(username, tab_id)`, so the
    /// actual session token never has to travel in a WebSocket URL query
    /// string. Opportunistically sweeps expired tickets while we already hold
    /// the write lock, instead of running a separate background task.
    async fn issue_ticket(&self, username: &str, tab_id: &str) -> String {
        self.reap_stale().await;
        let ticket = generate_ticket();
        let now = Instant::now();
        let mut tickets = self.tickets.write().await;
        tickets.retain(|_, (_, _, issued_at)| now.duration_since(*issued_at) < TICKET_TTL);
        tickets.insert(ticket.clone(), (username.to_string(), tab_id.to_string(), now));
        ticket
    }

    /// Validates and consumes a ticket, returning the `(username, tab_id)` it
    /// was issued for. Each ticket can only be redeemed once and expires after
    /// `TICKET_TTL`.
    async fn consume_ticket(&self, ticket: &str) -> Option<(String, String)> {
        let mut tickets = self.tickets.write().await;
        let (username, tab_id, issued_at) = tickets.remove(ticket)?;
        if issued_at.elapsed() < TICKET_TTL {
            Some((username, tab_id))
        } else {
            None
        }
    }
}

/// Generates a random alphanumeric ticket. Mirrors the approach used for
/// session tokens (`AuthService::generate_token`) without reaching into that
/// module, since this ticket is a much shorter-lived, narrower-purpose value.
fn generate_ticket() -> String {
    use rand::Rng;

    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    let mut buf = [0u8; 32];
    rng.fill_bytes(&mut buf);
    buf.iter().map(|b| CHARSET[(*b as usize) % CHARSET.len()] as char).collect()
}

/// Reads `/proc/<pid>/cwd` — the process's real working directory. Returns
/// `None` when the process is gone or the symlink is unresolvable.
fn resolve_pid_cwd(pid: u32) -> Option<std::path::PathBuf> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    if cwd.is_absolute() {
        Some(cwd)
    } else {
        None
    }
}

/// All descendant PIDs of `pid`, breadth-first, read from
/// `/proc/<pid>/task/<pid>/children` (Linux-only; empty elsewhere).
fn descendant_pids(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![pid];
    // Cap the walk: a terminal shell tree is at most a handful of levels.
    for _ in 0..6 {
        let mut next = Vec::new();
        for p in stack {
            if let Ok(children) = std::fs::read_to_string(format!("/proc/{p}/task/{p}/children")) {
                for tok in children.split_whitespace() {
                    if let Ok(cp) = tok.parse::<u32>() {
                        out.push(cp);
                        next.push(cp);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        stack = next;
    }
    out
}

/// Spawns a shell for `username` inside a fresh PTY and wires up the reader
/// and writer background threads that bridge it to async channels.
///
/// Spawns the shell as the target user without requiring a Linux password.
/// The homevault service user is granted passwordless sudo to start a login
/// shell for any homevault_admin member via /etc/sudoers.d/homevault:
///   homevault ALL=(%homevault_admin) NOPASSWD: /usr/bin/sudo -u * -i
/// `sudo -u <user> -i` simulates an initial login, giving the user's own
/// HOME, shell and environment.
fn spawn_session(username: &str) -> Result<Arc<PtySession>, String> {
    if !crate::setup::SetupService::is_valid_username(username) {
        return Err(format!("Invalid username for terminal session: {}", username));
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open pty: {e}"))?;

    let mut cmd = CommandBuilder::new("sudo");
    cmd.arg("-u");
    cmd.arg(username);
    cmd.arg("-i");
    cmd.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn shell: {e}"))?;
    // Drop the slave end now that the child owns it: keeping it open here would
    // prevent the master's reader from ever seeing EOF once the shell exits.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to clone pty reader: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to take pty writer: {e}"))?;

    let (output_tx, _) = broadcast::channel::<Bytes>(OUTPUT_CHANNEL_CAPACITY);
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Bytes>();
    let (ended_tx, ended_rx) = watch::channel(false);
    let scrollback: Arc<StdMutex<VecDeque<u8>>> =
        Arc::new(StdMutex::new(VecDeque::with_capacity(SCROLLBACK_CAPACITY)));

    // Reader thread: PTY output -> broadcast to every attached client, and
    // appended to the scrollback buffer for clients that attach later.
    {
        let output_tx = output_tx.clone();
        let scrollback = scrollback.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        {
                            let mut sb = scrollback.lock().unwrap();
                            sb.extend(&buf[..n]);
                            let overflow = sb.len().saturating_sub(SCROLLBACK_CAPACITY);
                            if overflow > 0 {
                                sb.drain(..overflow);
                            }
                        }
                        // Ignore send errors: no receivers just means nobody is
                        // currently attached, the shell keeps running regardless.
                        let _ = output_tx.send(Bytes::copy_from_slice(&buf[..n]));
                    }
                }
            }
            // The shell has exited (or the PTY broke): tell every attached
            // handle_socket so they can close the WebSocket cleanly instead
            // of leaving it open with nothing left behind it.
            let _ = ended_tx.send(true);
        });
    }

    // Writer thread: merged input from every attached client -> PTY.
    std::thread::spawn(move || {
        while let Some(data) = input_rx.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }
        }
    });

    tracing::info!("Started native terminal session for user {}", username);

    Ok(Arc::new(PtySession {
        master: Arc::new(StdMutex::new(pair.master)),
        child: Arc::new(StdMutex::new(child)),
        output_tx,
        input_tx,
        scrollback,
        ended_rx,
        attached: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        // A freshly-spawned tab starts its idle clock now, so one that is
        // created but never actually attached to still gets reaped.
        idle_since: Arc::new(StdMutex::new(Some(Instant::now()))),
    }))
}

/// Creates the terminal router serving the ticket-issuing and WebSocket
/// terminal endpoints.
pub fn create_terminal_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ticket", post(handle_terminal_ticket))
        .route("/close", post(handle_terminal_close))
        .route("/cwd", get(handle_terminal_cwd))
        .route("/ls", get(handle_terminal_ls))
        .route("/ws", get(terminal_ws_handler))
        .with_state(state)
}

#[derive(Deserialize)]
struct TerminalWsQuery {
    ticket: Option<String>,
}

/// `?tab=<opaque id>` on the ticket / close endpoints. Defaults to a single
/// shared id so an older cached client that sends nothing still works.
#[derive(Deserialize)]
struct TabQuery {
    tab: Option<String>,
}

fn resolve_tab_id(tab: Option<String>) -> Result<String, Response> {
    let tab_id = tab.unwrap_or_else(|| "default".to_string());
    if is_valid_tab_id(&tab_id) {
        Ok(tab_id)
    } else {
        Err((StatusCode::BAD_REQUEST, "Invalid terminal tab id").into_response())
    }
}

#[derive(Deserialize)]
struct ResizeMessage {
    cols: u16,
    rows: u16,
}

/// Issues a short-lived, single-use ticket that the client immediately
/// exchanges for a WebSocket connection. Keeps the long-lived session token
/// out of the WS URL (and therefore out of access/proxy logs).
async fn handle_terminal_ticket(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TabQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "No session token provided").into_response();
    }

    let Some(username) = state.auth.get_username_from_token(session_token).await else {
        return (StatusCode::UNAUTHORIZED, "Invalid session").into_response();
    };

    if !state.auth.is_admin(&username).await {
        return (StatusCode::FORBIDDEN, "Admin privileges required for terminal access").into_response();
    }

    let tab_id = match resolve_tab_id(query.tab) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let ticket = state.pty_manager.issue_ticket(&username, &tab_id).await;
    Json(serde_json::json!({ "success": true, "data": { "ticket": ticket } })).into_response()
}

/// The shell's live working directory for `?tab=` — feeds the UI's prompt
/// and path completion. `Ok(())` on unknown tab → `data.cwd: null`.
async fn handle_terminal_cwd(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TabQuery>,
    headers: HeaderMap,
) -> Response {
    let Ok(username) = authed_admin(&state, &headers).await else {
        return admin_gate_failure(&state, &headers).await;
    };
    let tab_id = match resolve_tab_id(query.tab) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let cwd = state.pty_manager.get_session_cwd(&username, &tab_id).await;
    Json(serde_json::json!({ "success": true, "data": { "cwd": cwd.map(|p| p.to_string_lossy().into_owned()) } }))
        .into_response()
}

#[derive(Deserialize)]
struct TerminalLsQuery {
    tab: Option<String>,
    /// Absolute directory to list.
    path: String,
}

#[derive(Serialize)]
struct LsEntry {
    name: String,
    is_dir: bool,
}

/// Directory listing for the completion popup. Admin-only (the terminal is
/// admin-only); the caller is the admin's own shell, so any absolute path is
/// acceptable, but the result is capped and pruned to plain names.
async fn handle_terminal_ls(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TerminalLsQuery>,
    headers: HeaderMap,
) -> Response {
    let Ok(username) = authed_admin(&state, &headers).await else {
        return admin_gate_failure(&state, &headers).await;
    };
    let tab_id = match resolve_tab_id(query.tab) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    // The caller must have a live session for this tab — an arbitrary admin
    // can't list directories unless they actually hold a terminal tab open.
    if state.pty_manager.get_session_cwd(&username, &tab_id).await.is_none() {
        return (StatusCode::NOT_FOUND, "No live terminal session for this tab").into_response();
    }

    let path = query.path.trim();
    let path_buf = std::path::Path::new(path);
    if path.is_empty() || !path_buf.is_absolute() {
        return (StatusCode::BAD_REQUEST, "path must be absolute").into_response();
    }
    if path.chars().any(|c| c.is_control()) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }
    if !path_buf.is_dir() {
        return (StatusCode::BAD_REQUEST, "Not a directory").into_response();
    }

    let mut entries = Vec::new();
    let Ok(rd) = std::fs::read_dir(path_buf) else {
        return (StatusCode::BAD_REQUEST, "Cannot read directory").into_response();
    };
    for item in rd.flatten().take(500) {
        let Ok(name) = item.file_name().into_string() else { continue };
        if name.is_empty() {
            continue;
        }
        let is_dir = item.path().is_dir();
        entries.push(LsEntry { name, is_dir });
    }
    // Directories first, then names — the same order a shell `ls` suggests.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    Json(serde_json::json!({ "success": true, "data": { "entries": entries } })).into_response()
}

/// Shared admin gate for the terminal's HTTP (non-WS) endpoints. `Ok` =
/// admin username; `Err(())` means "not an authenticated admin" — the exact
/// 401-vs-403 split is produced by `admin_gate_failure`, which re-reads the
/// headers so it can distinguish "no valid session" from "session but not
/// admin", matching the existing ticket/close endpoints.
async fn authed_admin(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, ()> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if session_token.is_empty() {
        return Err(());
    }
    let username = state.auth.get_username_from_token(session_token).await.ok_or(())?;
    if state.auth.is_admin(&username).await {
        Ok(username)
    } else {
        Err(())
    }
}

/// The concrete status code for a failed `authed_admin` call: 401 when there
/// is no valid session, 403 when the session belongs to a non-admin.
async fn admin_gate_failure(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let has_session = !session_token.is_empty()
        && state.auth.get_username_from_token(session_token).await.is_some();
    if has_session {
        (StatusCode::FORBIDDEN, "Admin privileges required").into_response()
    } else {
        (StatusCode::UNAUTHORIZED, "Invalid session").into_response()
    }
}

/// Explicitly tears down one shell tab. The WebSocket closing on its own is
/// deliberately *not* enough to kill the PTY (so a reconnecting tab keeps its
/// shell), so a tab the user actually closed in the UI needs this call.
async fn handle_terminal_close(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TabQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "No session token provided").into_response();
    }

    let Some(username) = state.auth.get_username_from_token(session_token).await else {
        return (StatusCode::UNAUTHORIZED, "Invalid session").into_response();
    };

    if !state.auth.is_admin(&username).await {
        return (StatusCode::FORBIDDEN, "Admin privileges required for terminal access").into_response();
    }

    let tab_id = match resolve_tab_id(query.tab) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    state.pty_manager.close_tab(&username, &tab_id).await;
    Json(serde_json::json!({ "success": true })).into_response()
}

async fn terminal_ws_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TerminalWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(ticket) = query.ticket.filter(|t| !t.is_empty()) else {
        return (StatusCode::UNAUTHORIZED, "No terminal ticket provided").into_response();
    };

    let Some((username, tab_id)) = state.pty_manager.consume_ticket(&ticket).await else {
        return (StatusCode::UNAUTHORIZED, "Invalid or expired terminal ticket").into_response();
    };

    // Defense in depth: re-check admin status at connect time even though the
    // ticket was only ever issued to an admin (matches the same pattern
    // handle_group_add_member uses for the admin group).
    if !state.auth.is_admin(&username).await {
        tracing::warn!("Non-admin user '{}' denied access to terminal", username);
        return (StatusCode::FORBIDDEN, "Admin privileges required for terminal access").into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state, username, tab_id))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>, username: String, tab_id: String) {
    let session = match state
        .pty_manager
        .get_or_create_session(&username, &tab_id)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                "Failed to start terminal session for {} (tab {}): {}",
                username,
                tab_id,
                e
            );
            return;
        }
    };

    // Count this attachment for the whole lifetime of the socket, however it
    // ends (early return below, loop break, or a panic unwinding through
    // here): the guard's Drop is the single place the count comes back down.
    session.mark_attached();
    struct DetachGuard(Arc<PtySession>);
    impl Drop for DetachGuard {
        fn drop(&mut self) {
            self.0.mark_detached();
        }
    }
    let _detach = DetachGuard(session.clone());

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut output_rx = session.output_tx.subscribe();
    let mut ended_rx = session.ended_rx.clone();

    // Replay recent output first, so a (re)attaching tab sees the last bit of
    // context instead of a blank screen while it waits for new live output.
    {
        let buffered: Vec<u8> = session.scrollback.lock().unwrap().iter().copied().collect();
        if !buffered.is_empty() && ws_tx.send(Message::Binary(Bytes::from(buffered))).await.is_err() {
            return;
        }
    }

    let input_tx = session.input_tx.clone();
    let master = session.master.clone();

    loop {
        tokio::select! {
            out = output_rx.recv() => {
                match out {
                    Ok(data) => {
                        if ws_tx.send(Message::Binary(data)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        let _ = input_tx.send(data);
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(resize) = serde_json::from_str::<ResizeMessage>(&text) {
                            if let Ok(master) = master.lock() {
                                let _ = master.resize(PtySize {
                                    rows: resize.rows,
                                    cols: resize.cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = ended_rx.changed() => {
                if *ended_rx.borrow() {
                    let _ = ws_tx.send(Message::Close(Some(CloseFrame {
                        code: 1000,
                        reason: "Shell session ended".into(),
                    }))).await;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the native-PTY terminal manager.
    //!
    //! Covers the pure gatekeeping logic (`is_valid_tab_id`, `resolve_tab_id`,
    //! `generate_ticket`) and the parts of `PtyManager` that don't spawn a
    //! real shell: the one-time connection-ticket lifecycle, and the cleanup
    //! methods (`close_tab` / `kill_session` / `cleanup_all` / `reap_stale`)
    //! on sessions that don't exist. Spawning an actual `sudo -u <user> -i`
    //! shell — and therefore the `MAX_TABS_PER_USER` cap and the idle
    //! reaper's kill path — needs the real host and is
    //! `tests/e2e/terminal/terminal.spec.ts`'s job.
    //!
    //! Run: `cargo test handlers::terminal_pty::tests`

    use super::*;

    // -- is_valid_tab_id -------------------------------------------------

    #[test]
    fn test_is_valid_tab_id_accepts_opaque_client_ids() {
        assert!(is_valid_tab_id("default"));
        assert!(is_valid_tab_id("tab-1"));
        assert!(is_valid_tab_id("tab_2"));
        assert!(is_valid_tab_id("aZ09-_"));
        assert!(is_valid_tab_id(&"x".repeat(64))); // exactly the cap
    }

    #[test]
    fn test_is_valid_tab_id_rejects_empty_overlong_and_odd_bytes() {
        assert!(!is_valid_tab_id(""));
        assert!(!is_valid_tab_id(&"x".repeat(65)));
        for bad in ["has space", "slash/id", "dot.id", "colon:id", "quote\"id", "emoji\u{1F600}", "a\nb"] {
            assert!(!is_valid_tab_id(bad), "{bad:?} must be rejected");
        }
    }

    // -- resolve_tab_id -----------------------------------------------

    #[test]
    fn test_resolve_tab_id_defaults_when_absent() {
        assert_eq!(resolve_tab_id(None).unwrap(), "default");
    }

    #[test]
    fn test_resolve_tab_id_passes_a_valid_id_through() {
        assert_eq!(resolve_tab_id(Some("tab-9".to_string())).unwrap(), "tab-9");
    }

    #[test]
    fn test_resolve_tab_id_rejects_an_invalid_id_with_400() {
        let resp = resolve_tab_id(Some("bad id!".to_string())).unwrap_err();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // -- generate_ticket -------------------------------------------

    #[test]
    fn test_generate_ticket_shape_and_uniqueness() {
        let a = generate_ticket();
        let b = generate_ticket();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, b, "two tickets must not collide");
    }

    // -- PtyManager: ticket lifecycle (no shell spawned) -----------

    #[tokio::test]
    async fn test_ticket_issue_then_consume_returns_the_pair() {
        let mgr = PtyManager::new();
        let ticket = mgr.issue_ticket("alice", "tab-1").await;
        assert_eq!(ticket.len(), 32);
        let got = mgr.consume_ticket(&ticket).await;
        assert_eq!(got, Some(("alice".to_string(), "tab-1".to_string())));
    }

    #[tokio::test]
    async fn test_ticket_is_single_use() {
        let mgr = PtyManager::new();
        let ticket = mgr.issue_ticket("bob", "default").await;
        assert!(mgr.consume_ticket(&ticket).await.is_some());
        assert!(
            mgr.consume_ticket(&ticket).await.is_none(),
            "a redeemed ticket must not work a second time"
        );
    }

    #[tokio::test]
    async fn test_consume_unknown_ticket_is_none() {
        let mgr = PtyManager::new();
        assert!(mgr.consume_ticket("not-a-real-ticket").await.is_none());
        assert!(mgr.consume_ticket("").await.is_none());
    }

    #[tokio::test]
    async fn test_issuing_many_tickets_keeps_them_all_valid_within_ttl() {
        let mgr = PtyManager::new();
        let tickets: Vec<String> =
            futures_util::future::join_all((0..10).map(|i| {
                let mgr = &mgr;
                async move { mgr.issue_ticket("carol", &format!("tab-{i}")).await }
            }))
            .await;
        for (i, t) in tickets.iter().enumerate() {
            assert_eq!(
                mgr.consume_ticket(t).await,
                Some(("carol".to_string(), format!("tab-{i}")))
            );
        }
    }

    // -- PtyManager: kill_session also drops pending tickets ------

    #[tokio::test]
    async fn test_kill_session_drops_that_users_unredeemed_tickets_only() {
        let mgr = PtyManager::new();
        let a1 = mgr.issue_ticket("dave", "t1").await;
        let a2 = mgr.issue_ticket("dave", "t2").await;
        let b1 = mgr.issue_ticket("erin", "t1").await;

        mgr.kill_session("dave").await;

        assert!(mgr.consume_ticket(&a1).await.is_none(), "dave's ticket must be gone");
        assert!(mgr.consume_ticket(&a2).await.is_none());
        assert_eq!(
            mgr.consume_ticket(&b1).await,
            Some(("erin".to_string(), "t1".to_string())),
            "erin's ticket must be untouched"
        );
    }

    // -- PtyManager: cleanup methods are safe no-ops when empty --

    #[tokio::test]
    async fn test_cleanup_methods_do_not_panic_on_an_empty_manager() {
        let mgr = PtyManager::new();
        mgr.close_tab("ghost", "nope").await;
        mgr.kill_session("ghost").await;
        mgr.reap_stale().await;
        mgr.cleanup_all().await;
        // still usable afterwards
        let t = mgr.issue_ticket("frank", "x").await;
        assert!(mgr.consume_ticket(&t).await.is_some());
    }

    // -- request-body models ------------------------------------

    #[test]
    fn test_resize_and_query_models_deserialize() {
        let r: ResizeMessage = serde_json::from_str(r#"{"cols":120,"rows":40}"#).unwrap();
        assert_eq!((r.cols, r.rows), (120, 40));

        let q: TerminalWsQuery =
            serde_json::from_str(r#"{"ticket":"abc"}"#).unwrap();
        assert_eq!(q.ticket.as_deref(), Some("abc"));
        let q2: TerminalWsQuery = serde_json::from_str("{}").unwrap();
        assert!(q2.ticket.is_none());
    }
}
