/* =============================================================================
 * File          : connected_clients.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   "Who is connected right now", across the protocols and not only the web
 *   UI: SMB sessions (smbstatus), NFS mounts (showmount), and the TCP streams
 *   of the managed services (ss). Pure parsers so the shapes are asserted
 *   without a live smbd; one collector that degrades per source; and a kick
 *   that re-collects before acting so it can never signal a pid a client
 *   supplied.
 *
 * Dependencies:
 *   - serde, crate::utils::process (bounded subprocesses), crate::shares
 *
 * Usage:
 *   handlers/connected_clients.rs (Security card); see docs/guides/security.md.
 * =============================================================================
 */

//! Connected clients across SMB / NFS / SFTP / FTP / WebDAV (see
//! docs/guides/security.md).
//!
//! "Active sessions" already existed for the **web UI** (`handlers::security`).
//! The weekly question is a different one — *why is the disk busy, and who is
//! copying?* — and its answer lives in the protocols, not in the browser.
//!
//! Design rules:
//!
//! * **One source failing never fails the list.** A box without Samba reports
//!   SMB as unavailable and still shows its NFS mounts. Each source carries its
//!   own status, so the card can say *why* a section is empty.
//! * **Every command is bounded** (`run_timed`, 5 s) and runs off the runtime
//!   (`spawn_blocking`): a wedged `smbstatus` cannot stall the request
//!   (ROADMAP X.2 doctrine).
//! * **A kick re-collects first.** The request names a client, never a pid: the
//!   pid is taken from the fresh collection, so a forged or stale id cannot
//!   signal an unrelated process on the box.
//! * **Only what can be kicked is kickable.** SMB (via `smbcontrol`) and the
//!   sshd/vsftpd sessions (via a signal to *that* connection's process) can;
//!   NFS cannot — the kernel holds it, and pretending otherwise would offer a
//!   button that fails.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Deadline for each collector command. `smbstatus -j` on a busy server is the
/// slowest of the three; five seconds is generous and still bounded.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Ports whose TCP streams count as "a client connected to this box".
///
/// The SMB port (445) and the FTP/SFTP ports are the ones where a *session
/// process* exists and can therefore be kicked. NFS traffic is kernel-side, so
/// it is read from `showmount` instead.
pub const SMB_PORT: u16 = 445;
pub const SMB_NETBIOS_PORT: u16 = 139;
pub const FTP_PORT: u16 = 21;
pub const SFTP_PORT: u16 = 22;

/// One client connection, as the card renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedClient {
    /// Stable identity for the kick request: `protocol:pid:remote`.
    pub id: String,
    /// `smb` | `nfs` | `sftp` | `ftp`.
    pub protocol: String,
    /// Account, or `-` when the protocol does not expose one.
    pub username: String,
    pub client_ip: String,
    pub client_host: Option<String>,
    /// Share name, export path, or `-`.
    pub target: String,
    pub opened_files: u32,
    /// RFC 3339 when the source reports it (SMB only so far).
    pub since: Option<String>,
    pub kickable: bool,
    /// Human-readable evidence ("smbd pid 4242"), never a credential.
    pub detail: String,
}

/// Status of one collector, so an empty section can explain itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source: String,
    pub available: bool,
    pub error: Option<String>,
    pub count: usize,
}

/// The whole answer: the clients plus the provenance of each list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedClients {
    pub clients: Vec<ConnectedClient>,
    pub sources: Vec<SourceStatus>,
}

impl ConnectedClients {
    /// How many clients can actually be disconnected from here.
    pub fn kickable(&self) -> usize {
        self.clients.iter().filter(|c| c.kickable).count()
    }
}

// --------------------------------------------------------------------------
// SMB — `smbstatus -j`
// --------------------------------------------------------------------------

/// Parse the JSON `smbstatus -j` prints.
///
/// Shape (Samba 4.x; values are strings in most builds and numbers in some, so
/// every field is read tolerantly):
///
/// ```json
/// {"sessions":{"1":{"session_id":"1","pid":"4242","username":"alice",
///                  "remote_machine":"192.168.1.20","hostname":"laptop"}},
///  "tcons":{"1":{"service":"photos","pid":"4242","machine":"192.168.1.20"}},
///  "open_files":{"1":{"service":"photos","pid":"4242","name":"/srv/photos/a.jpg"}}}
/// ```
///
/// Sessions are the rows; a session's shares come from `tcons` (matched by pid)
/// and its open-file count from `open_files`. A share name is not a secret, but
/// **no path from `open_files` is exposed** — only the count: the card answers
/// "who is connected", not "what are they reading".
pub fn parse_smbstatus_json(raw: &str) -> Result<Vec<ConnectedClient>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("smbstatus did not print JSON: {e}"))?;

    // pid -> (share services, open file count)
    let mut shares_by_pid: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    if let Some(tcons) = value.get("tcons").and_then(|v| v.as_object()) {
        for con in tcons.values() {
            let pid = text(con.get("pid"));
            let service = text(con.get("service"));
            if pid.is_empty() || service.is_empty() {
                continue;
            }
            shares_by_pid.entry(pid).or_default().push(service);
        }
    }
    let mut files_by_pid: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    if let Some(open) = value.get("open_files").and_then(|v| v.as_object()) {
        for file in open.values() {
            let pid = text(file.get("pid"));
            if !pid.is_empty() {
                *files_by_pid.entry(pid).or_insert(0) += 1;
            }
        }
    }

    let mut clients = Vec::new();
    if let Some(sessions) = value.get("sessions").and_then(|v| v.as_object()) {
        for session in sessions.values() {
            let pid = text(session.get("pid"));
            let username = text(session.get("username"));
            let remote = text(session.get("remote_machine"));
            if remote.is_empty() {
                continue;
            }
            // Samba echoes the peer address (sometimes `ipv4:`-decorated) in
            // `hostname` when it could not resolve a name. Compare against the
            // *normalised* address, or that echo is stored as a host name.
            let client_ip = normalise_ip(&remote);
            let hostname = session
                .get("hostname")
                .map(|value| text(Some(value)))
                .filter(|h| !h.is_empty() && h != &client_ip);
            let mut shares = shares_by_pid.get(&pid).cloned().unwrap_or_default();
            shares.sort();
            shares.dedup();
            let target = if shares.is_empty() {
                "-".to_string()
            } else {
                shares.join(", ")
            };
            let opened = files_by_pid.get(&pid).copied().unwrap_or(0);
            clients.push(ConnectedClient {
                id: format!("smb:{pid}:{remote}"),
                protocol: "smb".to_string(),
                username: if username.is_empty() {
                    "-".to_string()
                } else {
                    username
                },
                client_ip,
                client_host: hostname,
                target,
                opened_files: opened,
                since: None,
                // `smbcontrol close-share` needs a share and a pid; without a
                // share there is nothing to close (the session itself would need
                // `smbcontrol kill`, which drops the user's whole connection).
                kickable: !shares.is_empty() && !pid.is_empty(),
                detail: if pid.is_empty() {
                    "smbd".to_string()
                } else {
                    format!("smbd pid {pid}")
                },
            });
        }
    }
    clients.sort_by(|a, b| {
        a.client_ip
            .cmp(&b.client_ip)
            .then(a.username.cmp(&b.username))
    });
    Ok(clients)
}

/// `smbstatus`/JSON values are strings in most builds, numbers in some.
fn text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Strip an SMB/IPv6 decoration Samba sometimes adds (`ipv4:192.168.1.20`).
fn normalise_ip(value: &str) -> String {
    let trimmed = value.trim();
    for prefix in ["ipv4:", "ipv6:"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    trimmed.to_string()
}

// --------------------------------------------------------------------------
// NFS — `showmount -a`
// --------------------------------------------------------------------------

/// Parse `showmount -a` (`host:/export` per line).
///
/// NFS has no per-client session process, so these rows are **never kickable**:
/// the export is kernel-held, and "disconnecting" a client means changing the
/// export, not dropping a connection. Saying so is better than a button that
/// fails.
pub fn parse_showmount(raw: &str) -> Vec<ConnectedClient> {
    let mut clients = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("All mount points") {
            continue;
        }
        let Some((host, export)) = line.rsplit_once(':') else {
            continue;
        };
        let client_ip = normalise_ip(host);
        if client_ip.is_empty() {
            continue;
        }
        clients.push(ConnectedClient {
            id: format!("nfs:{export}:{client_ip}"),
            protocol: "nfs".to_string(),
            username: "-".to_string(),
            client_ip,
            client_host: None,
            target: export.to_string(),
            opened_files: 0,
            since: None,
            kickable: false,
            detail: "kernel export".to_string(),
        });
    }
    clients.sort_by(|a, b| a.client_ip.cmp(&b.client_ip).then(a.target.cmp(&b.target)));
    clients
}

// --------------------------------------------------------------------------
// Streams — `ss -tnp`
// --------------------------------------------------------------------------

/// One established inbound TCP stream of a managed service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundStream {
    pub local_port: u16,
    pub remote_ip: String,
    /// Process name as `ss` printed it (`smbd`, `sshd`, `vsftpd`).
    pub process: String,
    pub pid: Option<u32>,
}

/// Parse `ss -tnp` output, keeping established streams on `ports`.
///
/// A line looks like:
///
/// ```text
/// ESTAB 0 0 192.168.1.5:445 192.168.1.20:51234 users:(("smbd",pid=4242,fd=20))
/// ```
///
/// Unparsable lines are skipped rather than failing the list: `ss` output
/// changes between iproute2 releases and a new column must not blank the card.
pub fn parse_ss_streams(raw: &str, ports: &[u16]) -> Vec<InboundStream> {
    let mut streams = Vec::new();
    for line in raw.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        // `ss -H -tnp state established` omits the `State` column (the filter
        // makes it redundant), so the line starts at Recv-Q. `ss -H -tnp` keeps
        // the state, and a header line may still say `State`. Accept both
        // layouts instead of assuming one and silently collecting nothing.
        let (local_idx, peer_idx, process_idx) = if fields[0].eq_ignore_ascii_case("ESTAB") {
            (3, 4, 5)
        } else if fields[0].eq_ignore_ascii_case("State") {
            continue;
        } else if fields[0].parse::<u64>().is_ok() {
            (2, 3, 4)
        } else {
            // Any other state (CLOSE-WAIT, TIME-WAIT, …) is not an established
            // stream on a managed port.
            continue;
        };
        let (Some(local), Some(peer)) = (fields.get(local_idx), fields.get(peer_idx)) else {
            continue;
        };
        let Some((_, local_port)) = split_host_port(local) else {
            continue;
        };
        if !ports.contains(&local_port) {
            continue;
        }
        let Some((remote_ip, _)) = split_host_port(peer) else {
            continue;
        };
        let (process, pid) = parse_process(fields.get(process_idx).copied().unwrap_or(""));
        streams.push(InboundStream {
            local_port,
            remote_ip: normalise_ip(&remote_ip),
            process,
            pid,
        });
    }
    streams
}

/// `192.168.1.5:445` / `[fe80::1]:445` → `(host, 445)`.
fn split_host_port(value: &str) -> Option<(String, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?.parse::<u16>().ok()?;
        return Some((host.to_string(), port));
    }
    let (host, port) = value.rsplit_once(':')?;
    Some((host.to_string(), port.parse::<u16>().ok()?))
}

/// `users:(("smbd",pid=4242,fd=20))` → `("smbd", Some(4242))`.
fn parse_process(value: &str) -> (String, Option<u32>) {
    let Some(start) = value.find("((\"") else {
        return (String::new(), None);
    };
    let rest = &value[start + 3..];
    let name = rest.split('"').next().unwrap_or("").to_string();
    let pid = rest
        .split("pid=")
        .nth(1)
        .and_then(|tail| tail.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse::<u32>().ok());
    (name, pid)
}

/// Turn the SMB/SSH streams into client rows, merging duplicates per
/// (protocol, remote address, pid) so one session is one row.
pub fn clients_from_streams(streams: &[InboundStream]) -> Vec<ConnectedClient> {
    let mut seen = std::collections::HashSet::new();
    let mut clients = Vec::new();
    for stream in streams {
        let protocol = match stream.local_port {
            SMB_PORT | SMB_NETBIOS_PORT => "smb",
            SFTP_PORT => "sftp",
            FTP_PORT => "ftp",
            other => {
                // A port in the managed set that we do not otherwise describe:
                // keep it visible, but as a plain stream.
                let _ = other;
                "tcp"
            }
        };
        let pid_key = stream.pid.map(|p| p.to_string()).unwrap_or_default();
        let key = format!("{protocol}:{}:{pid_key}", stream.remote_ip);
        if !seen.insert(key.clone()) {
            continue;
        }
        clients.push(ConnectedClient {
            id: key,
            protocol: protocol.to_string(),
            username: "-".to_string(),
            client_ip: stream.remote_ip.clone(),
            client_host: None,
            target: "-".to_string(),
            opened_files: 0,
            since: None,
            // Only the session processes this box owns can be signalled. A
            // stream without a pid is a socket we cannot attribute.
            kickable: match (protocol, stream.pid) {
                ("smb", _) | ("sftp", Some(_)) | ("ftp", Some(_)) => stream.pid.is_some(),
                _ => false,
            },
            detail: match stream.pid {
                Some(pid) if !stream.process.is_empty() => format!("{} pid {pid}", stream.process),
                Some(pid) => format!("pid {pid}"),
                None => "stream".to_string(),
            },
        });
    }
    clients.sort_by(|a, b| {
        a.client_ip
            .cmp(&b.client_ip)
            .then(a.protocol.cmp(&b.protocol))
    });
    clients
}

// --------------------------------------------------------------------------
// Collecting
// --------------------------------------------------------------------------

/// Run one probe command, returning its stdout or a human reason.
///
/// Bounded and off the runtime: a missing binary is reported as such (the card
/// says "not installed"), not as an empty list.
fn probe(program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    cmd.env("LC_ALL", "C");
    match crate::utils::process::run_timed(&mut cmd, PROBE_TIMEOUT) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{program} exited with {}", output.status)
            } else {
                first_line(&stderr)
            })
        }
        Err(e) => Err(first_line(&e)),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 200 {
        line.chars().take(200).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

/// Collect every source, each independently.
///
/// Returns the clients plus one [`SourceStatus`] per source. Never fails: a box
/// with none of the tools installed answers with three unavailable sources and
/// an empty list, which is the truth.
pub async fn collect() -> ConnectedClients {
    // The three probes are blocking; run them on the blocking pool so the
    // request never holds a runtime thread (ROADMAP X.1).
    let (smb, nfs, streams) = tokio::task::spawn_blocking(|| {
        let smb = probe("smbstatus", &["-j"]);
        let nfs = probe("showmount", &["-a"]);
        let streams = probe("ss", &["-H", "-tnp", "state", "established"]);
        (smb, nfs, streams)
    })
    .await
    .unwrap_or_else(|e| {
        (
            Err(format!("the collector did not finish: {e}")),
            Err("the collector did not finish".to_string()),
            Err("the collector did not finish".to_string()),
        )
    });

    let mut clients = Vec::new();
    let mut sources = Vec::new();

    let push_source = |source: &str,
                       result: &Result<Vec<ConnectedClient>, String>,
                       sources: &mut Vec<SourceStatus>,
                       clients: &mut Vec<ConnectedClient>| {
        match result {
            Ok(list) => {
                sources.push(SourceStatus {
                    source: source.to_string(),
                    available: true,
                    error: None,
                    count: list.len(),
                });
                clients.extend(list.iter().cloned());
            }
            Err(e) => sources.push(SourceStatus {
                source: source.to_string(),
                available: false,
                error: Some(e.clone()),
                count: 0,
            }),
        }
    };

    let smb_result = smb
        .as_deref()
        .map_err(|e| e.to_string())
        .and_then(parse_smbstatus_json);
    push_source("smb", &smb_result, &mut sources, &mut clients);

    let nfs_result: Result<Vec<ConnectedClient>, String> = nfs
        .as_deref()
        .map(|raw| parse_showmount(raw))
        .map_err(|e| e.to_string());
    push_source("nfs", &nfs_result, &mut sources, &mut clients);

    let ports = [SMB_PORT, SMB_NETBIOS_PORT, SFTP_PORT, FTP_PORT];
    let streams_result: Result<Vec<ConnectedClient>, String> = streams
        .as_deref()
        .map(|raw| clients_from_streams(&parse_ss_streams(raw, &ports)))
        .map_err(|e| e.to_string());
    push_source("sessions", &streams_result, &mut sources, &mut clients);

    // The stream rows are the *evidence* a session exists (and carry the pid a
    // kick needs); the SMB rows carry the account and the share. Merge them so
    // the card shows one row per client, from the richest source.
    let clients = merge(clients);
    ConnectedClients { clients, sources }
}

/// Merge rows describing the same connection, preferring the richer row
/// (SMB's account + share) and keeping the pid evidence from the stream row.
pub fn merge(clients: Vec<ConnectedClient>) -> Vec<ConnectedClient> {
    let mut out: Vec<ConnectedClient> = Vec::new();
    for client in clients {
        if let Some(existing) = out.iter_mut().find(|c| {
            c.client_ip == client.client_ip
                && c.protocol == client.protocol
                && (c.username == client.username || c.username == "-" || client.username == "-")
                && (c.target == client.target || c.target == "-" || client.target == "-")
        }) {
            // Prefer a real account, a real share, the higher file count and the
            // pid-bearing detail.
            if existing.username == "-" && client.username != "-" {
                existing.username = client.username;
            }
            if existing.target == "-" && client.target != "-" {
                existing.target = client.target;
            }
            if existing.client_host.is_none() {
                existing.client_host = client.client_host.clone();
            }
            existing.opened_files = existing.opened_files.max(client.opened_files);
            if existing.detail == "stream" || !client.detail.contains("pid") {
                // keep the pid-bearing detail when we have one
                if !existing.detail.contains("pid") && client.detail.contains("pid") {
                    existing.detail = client.detail.clone();
                    existing.id = client.id.clone();
                    existing.kickable = client.kickable;
                }
            }
            continue;
        }
        out.push(client);
    }
    out.sort_by(|a, b| {
        a.client_ip
            .cmp(&b.client_ip)
            .then(a.protocol.cmp(&b.protocol))
    });
    out
}

// --------------------------------------------------------------------------
// Kicking
// --------------------------------------------------------------------------

/// Why a kick was refused. The API maps these to a 400 with the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KickError {
    /// The client is not in the current collection (stale or forged id).
    Unknown(String),
    /// The protocol has no per-client process to signal.
    NotKickable(String),
    /// The command failed; the reason is the tool's own.
    Failed(String),
}

impl KickError {
    pub fn message(&self) -> String {
        match self {
            KickError::Unknown(id) => {
                format!("No client '{id}' is connected right now — refresh the list and try again")
            }
            KickError::NotKickable(protocol) => format!(
                "A {protocol} connection cannot be disconnected from here: the export is held by \
                 the kernel. Change the export or block the address instead."
            ),
            KickError::Failed(reason) => reason.clone(),
        }
    }
}

/// Disconnect one client.
///
/// The request never supplies a pid: the collection is re-run and the row is
/// matched by its id, so the pid handed to `smbcontrol`/`kill` is one this box
/// just observed. That is the whole safety argument for this feature — a "kick"
/// endpoint that trusted a client-supplied pid would be a remote signal
/// primitive.
pub async fn kick(client_id: &str) -> Result<String, KickError> {
    let current = collect().await;
    let Some(client) = current.clients.iter().find(|c| c.id == client_id) else {
        return Err(KickError::Unknown(client_id.to_string()));
    };
    if !client.kickable {
        return Err(KickError::NotKickable(client.protocol.clone()));
    }
    let pid = client
        .detail
        .split("pid ")
        .nth(1)
        .and_then(|tail| tail.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse::<u32>().ok());

    match client.protocol.as_str() {
        "smb" => {
            let Some(pid) = pid else {
                return Err(KickError::Failed(
                    "this SMB session has no process to close".to_string(),
                ));
            };
            // `close-share` drops the share connection and keeps the session
            // (the user can reconnect); `kill` would drop every share of that
            // user, which is a different action nobody asked for.
            let share = client.target.split(',').next().unwrap_or("").trim();
            if share.is_empty() || share == "-" {
                return Err(KickError::Failed(
                    "this SMB session has no open share to close".to_string(),
                ));
            }
            probe("smbcontrol", &[&pid.to_string(), "close-share", share])
                .map(|_| {
                    format!(
                        "Closed share '{share}' for {} (session {pid})",
                        client.client_ip
                    )
                })
                .map_err(KickError::Failed)
        }
        "sftp" | "ftp" => {
            let Some(pid) = pid else {
                return Err(KickError::Failed(
                    "this session has no process to end".to_string(),
                ));
            };
            // Validate before signalling: the pid must be one of the session
            // processes we just collected (never an arbitrary number), and it
            // must not be this server itself.
            if pid == std::process::id() {
                return Err(KickError::Failed(
                    "refusing to signal this server's own process".to_string(),
                ));
            }
            let mut cmd = std::process::Command::new("kill");
            cmd.args(["-TERM", &pid.to_string()]);
            match crate::utils::process::run_timed_detached(&mut cmd, PROBE_TIMEOUT) {
                Ok(status) if status.success() => Ok(format!(
                    "Ended the {} session from {} (pid {pid})",
                    client.protocol.to_uppercase(),
                    client.client_ip
                )),
                Ok(status) => Err(KickError::Failed(format!(
                    "the signal to pid {pid} failed ({status})"
                ))),
                Err(e) => Err(KickError::Failed(first_line(&e))),
            }
        }
        other => Err(KickError::NotKickable(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_kick_error_explains_refresh_without_exposing_a_bare_id() {
        let id = "smb:999999:203.0.113.9";
        let message = KickError::Unknown(id.to_string()).message();
        assert!(message.contains("is connected right now"));
        assert!(message.contains("refresh"));
        assert!(message.contains(id));
        assert_ne!(message, id);
    }

    // ---------------------------------------------------------------------
    // SMB
    // ---------------------------------------------------------------------

    fn smbstatus_sample() -> String {
        serde_json::json!({
            "sessions": {
                "1": {
                    "session_id": "1",
                    "pid": "4242",
                    "uid": 1000,
                    "username": "alice",
                    "remote_machine": "192.168.1.20",
                    "hostname": "laptop"
                },
                "2": {
                    "session_id": "2",
                    "pid": 5151,
                    "uid": 1001,
                    "username": "bob",
                    "remote_machine": "ipv4:192.168.1.30",
                    "hostname": "192.168.1.30"
                }
            },
            "tcons": {
                "1": { "service": "photos", "pid": "4242", "machine": "192.168.1.20" },
                "2": { "service": "backups", "pid": 5151, "machine": "192.168.1.30" }
            },
            "open_files": {
                "1": { "service": "photos", "pid": "4242", "name": "/srv/photos/a.jpg" },
                "2": { "service": "photos", "pid": "4242", "name": "/srv/photos/b.jpg" },
                "3": { "service": "backups", "pid": 5151, "name": "/srv/backups/c.tar" }
            }
        })
        .to_string()
    }

    #[test]
    fn smb_sessions_become_one_row_each_with_their_share_and_count() {
        let clients = parse_smbstatus_json(&smbstatus_sample()).expect("parses");
        assert_eq!(clients.len(), 2);

        let alice = clients.iter().find(|c| c.username == "alice").unwrap();
        assert_eq!(alice.protocol, "smb");
        assert_eq!(alice.client_ip, "192.168.1.20");
        assert_eq!(alice.client_host.as_deref(), Some("laptop"));
        assert_eq!(alice.target, "photos");
        assert_eq!(alice.opened_files, 2, "two open files for that pid");
        assert_eq!(alice.detail, "smbd pid 4242");
        assert!(alice.kickable);
        // The identity a kick uses carries the pid, so a stale row cannot be
        // replayed against a different process.
        assert_eq!(alice.id, "smb:4242:192.168.1.20");

        // A numeric pid in the JSON is read the same way, and the `ipv4:`
        // decoration is stripped.
        let bob = clients.iter().find(|c| c.username == "bob").unwrap();
        assert_eq!(bob.client_ip, "192.168.1.30");
        assert_eq!(bob.opened_files, 1);
        // hostname == remote_machine is echoed by Samba; it is not a host name.
        assert_eq!(bob.client_host, None);
    }

    #[test]
    fn an_smb_session_without_a_share_is_not_kickable() {
        let raw = serde_json::json!({
            "sessions": {
                "1": { "pid": "9", "username": "carol", "remote_machine": "192.168.1.40" }
            }
        })
        .to_string();
        let clients = parse_smbstatus_json(&raw).expect("parses");
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].target, "-");
        assert!(
            !clients[0].kickable,
            "nothing to close: the user is authenticated but has no share open"
        );
    }

    #[test]
    fn no_file_path_from_smbstatus_ever_reaches_the_client_row() {
        // The card answers "who", not "what are they reading": a share name is
        // visible, an absolute path is not.
        let raw = smbstatus_sample();
        let clients = parse_smbstatus_json(&raw).unwrap();
        let rendered = serde_json::to_string(&clients).unwrap();
        assert!(!rendered.contains("/srv/photos"), "{rendered}");
        assert!(!rendered.contains("a.jpg"), "{rendered}");
    }

    #[test]
    fn smbstatus_that_is_not_json_is_an_explicit_error() {
        let error = parse_smbstatus_json("smbd is not running").unwrap_err();
        assert!(error.contains("did not print JSON"), "{error}");
    }

    #[test]
    fn an_empty_smbstatus_is_an_empty_list_not_an_error() {
        assert!(parse_smbstatus_json("{}").unwrap().is_empty());
        assert!(parse_smbstatus_json(r#"{"sessions":{}}"#)
            .unwrap()
            .is_empty());
    }

    // ---------------------------------------------------------------------
    // NFS
    // ---------------------------------------------------------------------

    #[test]
    fn showmount_lines_become_never_kickable_rows() {
        let raw = "All mount points on nas:\n\
                   192.168.1.20:/srv/photos\n\
                   192.168.1.30:/srv/backups\n\
                   \n";
        let clients = parse_showmount(raw);
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].protocol, "nfs");
        assert_eq!(clients[0].client_ip, "192.168.1.20");
        assert_eq!(clients[0].target, "/srv/photos");
        assert!(
            clients.iter().all(|c| !c.kickable),
            "a kernel-held export cannot be disconnected from here"
        );
        assert_eq!(clients[0].detail, "kernel export");
    }

    #[test]
    fn showmount_junk_lines_are_skipped() {
        let clients = parse_showmount("garbage\n\n:/no-host\nhost-without-colon\n");
        assert!(clients.is_empty(), "got {clients:?}");
    }

    // ---------------------------------------------------------------------
    // Streams
    // ---------------------------------------------------------------------

    #[test]
    fn ss_streams_are_filtered_to_the_managed_ports() {
        let raw = "\
ESTAB 0 0 192.168.1.5:445 192.168.1.20:51234 users:((\"smbd\",pid=4242,fd=20))
ESTAB 0 0 192.168.1.5:22 192.168.1.21:40000 users:((\"sshd\",pid=777,fd=3))
ESTAB 0 0 192.168.1.5:8080 192.168.1.22:40001 users:((\"homevault\",pid=1,fd=3))
CLOSE-WAIT 0 0 192.168.1.5:445 192.168.1.23:50000 users:((\"smbd\",pid=99,fd=20))
";
        let streams = parse_ss_streams(raw, &[SMB_PORT, SFTP_PORT]);
        assert_eq!(
            streams.len(),
            2,
            "only established streams on managed ports"
        );
        assert_eq!(streams[0].local_port, 445);
        assert_eq!(streams[0].remote_ip, "192.168.1.20");
        assert_eq!(streams[0].process, "smbd");
        assert_eq!(streams[0].pid, Some(4242));
        assert_eq!(streams[1].local_port, 22);
        assert_eq!(streams[1].process, "sshd");
    }

    #[test]
    fn ipv6_streams_and_header_lines_are_handled() {
        let raw = "\
State Recv-Q Send-Q Local Address:Port Peer Address:Port Process
ESTAB 0 0 [fe80::1]:445 [fe80::2]:52000 users:((\"smbd\",pid=10,fd=20))
";
        let streams = parse_ss_streams(raw, &[SMB_PORT]);
        assert_eq!(streams.len(), 1, "the header row is skipped");
        assert_eq!(streams[0].remote_ip, "fe80::2");
        assert_eq!(streams[0].pid, Some(10));
    }

    #[test]
    fn a_stream_without_a_process_is_visible_but_not_kickable() {
        let raw = "ESTAB 0 0 192.168.1.5:445 192.168.1.20:51234\n";
        let streams = parse_ss_streams(raw, &[SMB_PORT]);
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].pid, None);
        let clients = clients_from_streams(&streams);
        assert_eq!(clients.len(), 1);
        assert!(!clients[0].kickable, "no pid means nothing we may signal");
        assert_eq!(clients[0].detail, "stream");
    }

    #[test]
    fn one_session_on_several_streams_is_one_row() {
        let raw = "\
ESTAB 0 0 192.168.1.5:445 192.168.1.20:51234 users:((\"smbd\",pid=4242,fd=20))
ESTAB 0 0 192.168.1.5:445 192.168.1.20:51235 users:((\"smbd\",pid=4242,fd=21))
";
        let clients = clients_from_streams(&parse_ss_streams(raw, &[SMB_PORT]));
        assert_eq!(clients.len(), 1, "same protocol, address and pid");
        assert_eq!(clients[0].id, "smb:192.168.1.20:4242");
        assert!(clients[0].kickable);
    }

    // ---------------------------------------------------------------------
    // Merging
    // ---------------------------------------------------------------------

    #[test]
    fn the_smb_row_wins_the_account_and_the_stream_row_the_pid() {
        // smbstatus knows the account and the share; `ss` knows the pid of the
        // stream. The card must show both, in one row.
        let smb = parse_smbstatus_json(&smbstatus_sample()).unwrap();
        let streams = clients_from_streams(&parse_ss_streams(
            "ESTAB 0 0 192.168.1.5:445 192.168.1.20:51234 users:((\"smbd\",pid=4242,fd=20))\n",
            &[SMB_PORT],
        ));
        let merged = merge([smb, streams].concat());
        let alice: Vec<_> = merged
            .iter()
            .filter(|c| c.client_ip == "192.168.1.20")
            .collect();
        assert_eq!(alice.len(), 1, "one row per connection: {merged:?}");
        assert_eq!(alice[0].username, "alice");
        assert_eq!(alice[0].target, "photos");
        assert_eq!(alice[0].opened_files, 2);
        assert!(alice[0].detail.contains("pid 4242"), "{}", alice[0].detail);
    }

    #[test]
    fn merging_leaves_unrelated_rows_alone() {
        let a = ConnectedClient {
            id: "nfs:/srv/x:10.0.0.1".into(),
            protocol: "nfs".into(),
            username: "-".into(),
            client_ip: "10.0.0.1".into(),
            client_host: None,
            target: "/srv/x".into(),
            opened_files: 0,
            since: None,
            kickable: false,
            detail: "kernel export".into(),
        };
        let b = ConnectedClient {
            id: "smb:1:10.0.0.2".into(),
            protocol: "smb".into(),
            username: "dave".into(),
            client_ip: "10.0.0.2".into(),
            client_host: None,
            target: "docs".into(),
            opened_files: 0,
            since: None,
            kickable: true,
            detail: "smbd pid 1".into(),
        };
        let merged = merge(vec![a.clone(), b.clone()]);
        assert_eq!(merged.len(), 2);
        // Different protocols on the same address stay separate rows.
        let mut same_ip = a.clone();
        same_ip.protocol = "sftp".into();
        same_ip.id = "sftp:10.0.0.1".into();
        assert_eq!(merge(vec![a, same_ip]).len(), 2);
    }

    // ---------------------------------------------------------------------
    // Kick refusals (no process is ever signalled in a unit test)
    // ---------------------------------------------------------------------

    #[test]
    fn an_unknown_client_is_refused_with_the_reason() {
        let error = KickError::Unknown("smb:1:10.0.0.9".to_string());
        assert!(error.message().contains("is connected right now"));
    }

    #[test]
    fn an_nfs_client_explains_why_it_cannot_be_disconnected() {
        let error = KickError::NotKickable("nfs".to_string());
        let message = error.message();
        assert!(message.contains("kernel"), "{message}");
        assert!(
            message.contains("Change the export") || message.contains("block the address"),
            "a refusal must say what to do instead: {message}"
        );
    }

    #[test]
    fn the_kickable_count_matches_the_rows() {
        let collected = ConnectedClients {
            clients: vec![
                ConnectedClient {
                    id: "smb:1:10.0.0.1".into(),
                    protocol: "smb".into(),
                    username: "a".into(),
                    client_ip: "10.0.0.1".into(),
                    client_host: None,
                    target: "docs".into(),
                    opened_files: 0,
                    since: None,
                    kickable: true,
                    detail: "smbd pid 1".into(),
                },
                ConnectedClient {
                    id: "nfs:/srv:10.0.0.2".into(),
                    protocol: "nfs".into(),
                    username: "-".into(),
                    client_ip: "10.0.0.2".into(),
                    client_host: None,
                    target: "/srv".into(),
                    opened_files: 0,
                    since: None,
                    kickable: false,
                    detail: "kernel export".into(),
                },
            ],
            sources: vec![],
        };
        assert_eq!(collected.kickable(), 1);
    }
}
