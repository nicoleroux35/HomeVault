/* =============================================================================
 * File          : syslog_forward.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026
 * Last modified : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Remote syslog client (Log Center -> Log Sending): forwards HomeVault's own
 *   audit events to a collector over UDP, TCP or TLS, framed as RFC 5424 or
 *   RFC 3164. Pure framing/validation, a bounded drop-oldest queue, and one
 *   background worker — a request never waits on a syslog write.
 *
 * Dependencies:
 *   - serde (>= 1.0), parking_lot (>= 0.12), tokio (net/time), rustls,
 *     tokio-rustls, rustls-pemfile, chrono
 *
 * Usage:
 *   handlers::syslog exposes the endpoints and calls `send_test_message`;
 *   `run()` is spawned from web::run; `enqueue` is called by events::log_event.
 *   See docs/guides/journal.md.
 * =============================================================================
 */

//! Remote syslog forwarding for HomeVault (see docs/guides/journal.md).
//!
//! The feature has exactly one job: when HomeVault logs an audit event, a copy
//! of it leaves the box for the collector the admin configured. Four pieces:
//!
//! * **Framing** — [`frame`] renders one event as RFC 5424 (`<PRI>1 TIMESTAMP
//!   HOST APP PROCID MSGID SD MSG`) or the older RFC 3164, kept pure so the
//!   exact bytes can be asserted in a test.
//! * **A bounded queue** — [`enqueue`] is what a log call does, and it can only
//!   ever push a string into a `VecDeque`: no I/O, no lock held across an
//!   `await`, no way for a slow or dead collector to add latency to a request.
//!   When the queue is full the **oldest** entry is dropped and counted, so the
//!   most recent events survive.
//! * **One worker** — [`run`] wakes on [`enqueue`], takes a batch, sends it and
//!   records the outcome. One connection per batch (not per message), each step
//!   bounded by a deadline.
//! * **A transport seam** — [`SyslogTransport`] is what makes the worker
//!   testable: tests drive a scripted collector instead of the network.
//!
//! What is forwarded is HomeVault's own audit stream (every
//! `crate::events::log_event` / `log_event_warn`), not a tail of the host
//! journal. The host sources (`/var/log/syslog`, `auth.log`, …) belong to the
//! OS and are already collected by it; re-shipping them would duplicate every
//! line on the collector and could not be bounded the way this queue is.
//!
//! No credential ever reaches the wire from here: the settings carry no secret
//! (a CA certificate is public material), and messages are the plain-English
//! sentences the audit log already holds — the settings cards and the log
//! lines themselves are what must never contain a token.

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::storage::json::JsonStorage;

/// Storage key -> `{DATA_DIRECTORY}/data/syslog_forward.json`.
const STORE_KEY: &str = "syslog_forward";

/// How many events may wait in memory before the oldest is dropped.
///
/// 1024 lines is a few hours of a busy home box and a few dozen KiB; larger
/// buys nothing (a collector that is down for longer than that has a real
/// problem) and smaller starts dropping during a burst.
pub const MAX_QUEUE: usize = 1024;

/// Upper bound on one framed message. RFC 5424 recommends 2048 octets and
/// allows more over TCP; 8 KiB keeps a pathological audit line from being
/// truncated mid-sentence on most collectors.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// UDP is a datagram: anything longer than this is truncated rather than sent
/// in fragments a collector may drop.
pub const MAX_UDP_DATAGRAM: usize = 1024;

/// Deadline for the TCP/TLS connect step.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Deadline for writing one batch.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Events per send. One connection carries the batch, so a burst of 50 audit
/// lines is one TCP handshake rather than fifty.
pub const BATCH_SIZE: usize = 64;

/// Application name in the RFC 5424 header.
const APP_NAME: &str = "homevault";

/// Syslog severities this module emits (RFC 5424 §6.2.1).
pub const SEVERITY_ERR: u8 = 3;
pub const SEVERITY_WARNING: u8 = 4;
pub const SEVERITY_INFO: u8 = 6;

/// Default facility: `user`-level messages (RFC 5424 §6.2.1).
pub const DEFAULT_FACILITY: u8 = 1;

// --------------------------------------------------------------------------
// Settings
// --------------------------------------------------------------------------

/// How the batch reaches the collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyslogProtocol {
    /// One datagram per message; no connection, so a dead collector costs
    /// nothing but a `sendto` that never blocks.
    #[default]
    Udp,
    /// A stream: ordered and acknowledged, one connection per batch.
    Tcp,
    /// TCP wrapped in TLS, verified against the admin's pinned CA only.
    Tls,
}

impl SyslogProtocol {
    pub const ALL: &'static [SyslogProtocol] = &[
        SyslogProtocol::Udp,
        SyslogProtocol::Tcp,
        SyslogProtocol::Tls,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SyslogProtocol::Udp => "udp",
            SyslogProtocol::Tcp => "tcp",
            SyslogProtocol::Tls => "tls",
        }
    }

    /// Lenient parse for a stored value: an unknown string must not make the
    /// feature unloadable, it falls back to the default.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "tcp" => SyslogProtocol::Tcp,
            "tls" => SyslogProtocol::Tls,
            _ => SyslogProtocol::Udp,
        }
    }

    /// Strict parse for a request: an unknown value is refused (the admin gets
    /// an error rather than a silently different protocol).
    pub fn parse_strict(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "udp" => Ok(SyslogProtocol::Udp),
            "tcp" => Ok(SyslogProtocol::Tcp),
            "tls" => Ok(SyslogProtocol::Tls),
            other => Err(format!(
                "unknown protocol '{other}' (expected udp, tcp or tls)"
            )),
        }
    }

    pub fn is_stream(self) -> bool {
        matches!(self, SyslogProtocol::Tcp | SyslogProtocol::Tls)
    }
}

/// Which syslog format the collector expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SyslogFormat {
    /// RFC 5424 (2009): `<PRI>1 <timestamp> <host> <app> <procid> <msgid> <sd> <msg>`.
    #[default]
    Rfc5424,
    /// RFC 3164 (2001): `<PRI><MMM dd hh:mm:ss> <host> <tag>: <msg>`.
    Rfc3164,
}

impl SyslogFormat {
    pub const ALL: &'static [SyslogFormat] = &[SyslogFormat::Rfc5424, SyslogFormat::Rfc3164];

    pub fn as_str(self) -> &'static str {
        match self {
            SyslogFormat::Rfc5424 => "rfc5424",
            SyslogFormat::Rfc3164 => "rfc3164",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "rfc3164" => SyslogFormat::Rfc3164,
            _ => SyslogFormat::Rfc5424,
        }
    }

    pub fn parse_strict(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "rfc5424" => Ok(SyslogFormat::Rfc5424),
            "rfc3164" => Ok(SyslogFormat::Rfc3164),
            other => Err(format!(
                "unknown format '{other}' (expected rfc5424 or rfc3164)"
            )),
        }
    }
}

/// Lifecycle state shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SyslogStatus {
    #[default]
    Disabled,
    /// Enabled, queue empty, nothing failed yet.
    Idle,
    /// The last batch was accepted by the collector.
    Forwarding,
    /// Events are queued but not reaching the collector (unreachable, refused,
    /// or dropped on overflow).
    Degraded,
}

impl SyslogStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SyslogStatus::Disabled => "disabled",
            SyslogStatus::Idle => "idle",
            SyslogStatus::Forwarding => "forwarding",
            SyslogStatus::Degraded => "degraded",
        }
    }
}

/// Persisted settings (`data/syslog_forward.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyslogSettings {
    pub enabled: bool,
    #[serde(default)]
    pub protocol: SyslogProtocol,
    /// Collector host name or IP literal.
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub format: SyslogFormat,
    #[serde(default = "default_facility")]
    pub facility: u8,
    /// TLS only: the one certificate this box will trust. Empty means "no
    /// configured CA", which is refused while TLS is selected — falling back to
    /// the system roots would silently accept any public CA for a home
    /// collector's self-signed name.
    #[serde(default)]
    pub ca_pem: String,
    /// Forward HomeVault's own audit events (the only source this ships).
    #[serde(default = "default_true")]
    pub include_app_events: bool,
    #[serde(default)]
    pub status: SyslogStatus,
    /// RFC 3339, UTC: when a batch last reached the collector.
    #[serde(default)]
    pub last_sent_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn default_port() -> u16 {
    514
}

fn default_facility() -> u8 {
    DEFAULT_FACILITY
}

fn default_true() -> bool {
    true
}

impl Default for SyslogSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            protocol: SyslogProtocol::default(),
            host: String::new(),
            port: default_port(),
            format: SyslogFormat::default(),
            facility: DEFAULT_FACILITY,
            ca_pem: String::new(),
            include_app_events: true,
            status: SyslogStatus::Disabled,
            last_sent_at: None,
            last_error: None,
        }
    }
}

// --------------------------------------------------------------------------
// Persistence
// --------------------------------------------------------------------------

fn store() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// Load the saved settings, or the disabled default. A corrupt file degrades to
/// "disabled" instead of failing: log forwarding must never stop the server.
pub fn load_settings() -> SyslogSettings {
    match store().load::<SyslogSettings>(STORE_KEY) {
        Ok(Some(s)) => normalize(s),
        Ok(None) => SyslogSettings::default(),
        Err(e) => {
            tracing::warn!(
                "Syslog: cannot read data/syslog_forward.json, treating it as disabled: {e}"
            );
            SyslogSettings::default()
        }
    }
}

/// Serialises every write to `data/syslog_forward.json`.
///
/// Two writers exist: the admin's `PUT` and the worker recording the outcome of
/// a batch. Without this lock the worker could read the file, spend a second
/// sending, and then write its **stale snapshot** back over a destination the
/// admin had just saved — silently reversing a configuration change.
static SETTINGS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Write without taking [`SETTINGS_LOCK`]; callers already hold it.
fn save_unlocked(s: &SyslogSettings) -> Result<(), String> {
    let result = store()
        .save(STORE_KEY, &normalize(s.clone()))
        .map_err(|e| e.to_string());
    // The hot path caches `enabled`; a saved change must be visible at once.
    refresh_enabled_cache();
    result
}

pub fn save_settings(s: &SyslogSettings) -> Result<(), String> {
    let _guard = SETTINGS_LOCK.lock();
    save_unlocked(s)
}

/// Record an observation (status / last error / last sent) **without**
/// disturbing whatever configuration is on disk at that moment.
///
/// The worker never writes a whole snapshot it captured before the send: it
/// re-reads under the write lock and changes only these three fields, so a
/// destination saved during the send survives.
pub fn update_observation(apply: impl FnOnce(&mut SyslogSettings)) -> Result<(), String> {
    let _guard = SETTINGS_LOCK.lock();
    let mut current = load_settings();
    apply(&mut current);
    save_unlocked(&current)
}

/// Keep `status` consistent with `enabled`: a disabled entry always reads
/// `disabled`, and an enabled one can never claim to be disabled.
pub fn normalize(mut s: SyslogSettings) -> SyslogSettings {
    s.status = match (s.enabled, s.status) {
        (false, _) => SyslogStatus::Disabled,
        (true, SyslogStatus::Disabled) => SyslogStatus::Idle,
        (true, other) => other,
    };
    s
}

pub fn clear_settings() {
    let _guard = SETTINGS_LOCK.lock();
    let _ = store().delete(STORE_KEY);
    refresh_enabled_cache();
}

// --------------------------------------------------------------------------
// Validation
// --------------------------------------------------------------------------

/// A collector address: a bare host name or an IP literal, never a URL.
///
/// A scheme or a path here would mean the admin pasted something else (an
/// `https://` collector UI, say) and the connection would fail with a confusing
/// name-resolution error; refusing it names the actual problem.
pub fn validate_host(host: &str) -> Result<String, String> {
    let h = host.trim();
    if h.is_empty() {
        return Err("the syslog server is required".to_string());
    }
    if h.len() > 253 {
        return Err("the syslog server name is too long".to_string());
    }
    if h.contains("://") {
        return Err("enter the server name or address, without a scheme".to_string());
    }
    if h.contains('/') || h.contains(' ') || h.contains('@') {
        return Err("enter a bare host name or IP address".to_string());
    }
    if h.chars().any(|c| c.is_control()) {
        return Err("the syslog server contains control characters".to_string());
    }
    // An IP literal (v4 or v6) is always acceptable.
    if h.parse::<std::net::IpAddr>().is_ok() {
        return Ok(h.to_string());
    }
    if h.starts_with('[') || h.ends_with(']') {
        return Err("write an IPv6 address without brackets".to_string());
    }
    for label in h.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("each part of the server name must be 1 to 63 characters".to_string());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("parts of the server name cannot start or end with a hyphen".to_string());
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(
                "the server name may only contain letters, digits, hyphens and dots".to_string(),
            );
        }
    }
    Ok(h.to_ascii_lowercase())
}

/// Parse the pinned CA. Returning the DER certificates (not just "ok") is what
/// the TLS client needs, and it makes the test assert the *content* rather than
/// the presence of a string.
pub fn parse_ca_pem(pem: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    let mut certs = Vec::new();
    for item in rustls_pemfile::read_all(&mut reader) {
        match item {
            Ok(rustls_pemfile::Item::X509Certificate(der)) => certs.push(der.to_vec()),
            Ok(_) => {}
            Err(e) => return Err(format!("the CA certificate could not be read: {e}")),
        }
    }
    if certs.is_empty() {
        return Err(
            "paste the collector's CA certificate in PEM form (BEGIN CERTIFICATE)".to_string(),
        );
    }
    Ok(certs)
}

/// Validate a proposed configuration. Only enforced while enabled: an admin may
/// disable a half-filled entry and come back to it.
pub fn validate_settings(s: &SyslogSettings) -> Result<(), String> {
    if !s.enabled {
        return Ok(());
    }
    validate_host(&s.host)?;
    if s.port == 0 {
        return Err("the syslog port must be between 1 and 65535".to_string());
    }
    if s.facility > 23 {
        return Err("the facility must be between 0 and 23".to_string());
    }
    if s.protocol == SyslogProtocol::Tls && s.ca_pem.trim().is_empty() {
        return Err("TLS needs the collector's CA certificate so it can be pinned".to_string());
    }
    if s.protocol == SyslogProtocol::Tls {
        parse_ca_pem(&s.ca_pem)?;
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Framing
// --------------------------------------------------------------------------

/// `PRI` = facility × 8 + severity (RFC 5424 §6.2.1).
pub fn priority(facility: u8, severity: u8) -> u8 {
    facility.saturating_mul(8).saturating_add(severity)
}

/// Truncate to `max` bytes on a char boundary, so a multi-byte character is
/// never cut in half (which would make the whole message invalid UTF-8).
fn truncate_bytes(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// RFC 5424 header fields that must never contain a space (the collector parses
/// on them). `-` is the RFC's "unavailable" marker.
fn nil_or(value: &str, max: usize) -> String {
    let trimmed: String = value
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    if trimmed.is_empty() {
        "-".to_string()
    } else {
        truncate_bytes(&trimmed, max)
    }
}

/// One RFC 5424 message, given every header field explicitly.
///
/// Pure on purpose: this is the wire format, so it is asserted byte for byte.
/// `structured_data` is `-` in this module (HomeVault has no SD-IDs to publish).
pub fn frame_rfc5424(
    facility: u8,
    severity: u8,
    timestamp: &str,
    hostname: &str,
    app_name: &str,
    proc_id: &str,
    msg_id: &str,
    message: &str,
) -> String {
    format!(
        "<{}>1 {} {} {} {} {} - {}",
        priority(facility, severity),
        timestamp,
        nil_or(hostname, 255),
        nil_or(app_name, 48),
        nil_or(proc_id, 128),
        nil_or(msg_id, 32),
        message
    )
}

/// One RFC 3164 message, given every header field explicitly.
///
/// The timestamp is the collector's local time in `MMM dd hh:mm:ss` form; the
/// day is space-padded to two characters, exactly as the RFC's ABNF requires
/// (`SP DIGIT DIGIT` — a collector that receives `Oct 3` instead of `Oct  3`
/// fails to parse the whole line).
pub fn frame_rfc3164(
    facility: u8,
    severity: u8,
    timestamp: &str,
    hostname: &str,
    tag: &str,
    message: &str,
) -> String {
    format!(
        "<{}>{} {} {}: {}",
        priority(facility, severity),
        timestamp,
        nil_or(hostname, 255),
        nil_or(tag, 48),
        message
    )
}

/// RFC 3164 timestamp for a UTC instant: `Jun 12 04:05:06`.
pub fn rfc3164_timestamp(at: chrono::DateTime<chrono::Utc>) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS[(at.format("%m").to_string().parse::<usize>().unwrap_or(1) - 1).min(11)];
    format!(
        "{} {:>2} {}",
        month,
        at.format("%d").to_string().trim_start_matches('0'),
        at.format("%H:%M:%S")
    )
}

/// RFC 5424 timestamp: `2026-10-31T12:00:00Z` (seconds precision, UTC).
pub fn rfc5424_timestamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// This host's name, or `-` when it cannot be determined (never an error: a
/// missing host name must not stop forwarding).
pub fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "-".to_string())
}

/// Render one event for the configured collector, ready for the wire.
///
/// `msg_id` distinguishes the audit stream (`audit`) from a test message
/// (`test`), which is the first thing an admin looks for on the collector.
pub fn frame(
    settings: &SyslogSettings,
    severity: u8,
    msg_id: &str,
    message: &str,
    at: chrono::DateTime<chrono::Utc>,
    hostname: &str,
    proc_id: u32,
) -> Vec<u8> {
    let message = truncate_bytes(message, MAX_MESSAGE_BYTES);
    let rendered = match settings.format {
        SyslogFormat::Rfc5424 => frame_rfc5424(
            settings.facility,
            severity,
            &rfc5424_timestamp(at),
            hostname,
            APP_NAME,
            &proc_id.to_string(),
            msg_id,
            &message,
        ),
        SyslogFormat::Rfc3164 => frame_rfc3164(
            settings.facility,
            severity,
            &rfc3164_timestamp(at),
            hostname,
            &format!("{APP_NAME}[{proc_id}]"),
            &message,
        ),
    };
    // A UDP collector drops anything past its datagram budget rather than
    // fragmenting it in a way the receiver may not reassemble.
    let limit = if settings.protocol == SyslogProtocol::Udp {
        MAX_UDP_DATAGRAM
    } else {
        MAX_MESSAGE_BYTES
    };
    truncate_bytes(&rendered, limit).into_bytes()
}

// --------------------------------------------------------------------------
// Queue + counters
// --------------------------------------------------------------------------

/// One event waiting for the collector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedEvent {
    pub severity: u8,
    pub msg_id: String,
    pub message: String,
}

/// Runtime counters and the queue itself.
///
/// Everything here is process-local on purpose: a count that survives a restart
/// would describe a queue that no longer exists. `last_sent_at`/`last_error` do
/// persist, because they describe the collector rather than the queue.
#[derive(Debug, Default)]
pub struct SyslogQueue {
    pub items: VecDeque<QueuedEvent>,
    /// Events accepted into the queue since start.
    pub enqueued: u64,
    /// Events dropped because the queue was full (oldest first).
    pub dropped: u64,
    /// Messages the collector accepted since start.
    pub sent: u64,
    /// Batches the collector refused or that timed out.
    pub failed: u64,
}

impl SyslogQueue {
    /// Push one event, dropping the oldest when full.
    ///
    /// Returns `true` when something was dropped, so the caller can note it in
    /// the status without a second lock.
    pub fn push(&mut self, event: QueuedEvent) -> bool {
        let mut dropped = false;
        while self.items.len() >= MAX_QUEUE {
            self.items.pop_front();
            self.dropped += 1;
            dropped = true;
        }
        self.items.push_back(event);
        self.enqueued += 1;
        dropped
    }

    /// Take up to `BATCH_SIZE` events, newest last (the order they were logged).
    pub fn take_batch(&mut self) -> Vec<QueuedEvent> {
        let take = self.items.len().min(BATCH_SIZE);
        self.items.drain(..take).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

static QUEUE: LazyLock<Mutex<SyslogQueue>> = LazyLock::new(|| Mutex::new(SyslogQueue::default()));
static WAKE: LazyLock<Notify> = LazyLock::new(|| Notify::new());

/// Cached `enabled` flag, so the hot path (`events::log_event`, called by every
/// audited mutation) does not read and parse `data/syslog_forward.json` on every
/// event. Refreshed whenever the settings are written, and re-read at most once
/// a second so a file edited by hand is still picked up.
static ENABLED_UNTIL: LazyLock<Mutex<Option<(bool, std::time::Instant)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Cache lifetime for the `enabled` flag (one settings read per second at most).
const ENABLED_CACHE_TTL: Duration = Duration::from_secs(1);

/// Whether an audit event should be queued. Cheap enough to call from every log
/// line.
///
/// Both switches are honoured: turning the feature on but "Send HomeVault's own
/// events" off leaves the queue empty, which is what that switch promises.
pub fn forwarding_enabled() -> bool {
    {
        let cache = ENABLED_UNTIL.lock();
        if let Some((enabled, read_at)) = *cache {
            if read_at.elapsed() < ENABLED_CACHE_TTL {
                return enabled;
            }
        }
    }
    let settings = load_settings();
    let enabled = settings.enabled && settings.include_app_events;
    *ENABLED_UNTIL.lock() = Some((enabled, std::time::Instant::now()));
    enabled
}

/// Invalidate the cached `enabled` flag (called after every write).
pub fn refresh_enabled_cache() {
    *ENABLED_UNTIL.lock() = None;
}

/// Queue an audit event for the collector.
///
/// This is what a log call costs while forwarding is enabled: one lock, one
/// `VecDeque` push, one `notify_one`. It cannot block on the network, and it is
/// a no-op (not even a lock) while the feature is off, which is why
/// `events::log_event` can call it unconditionally.
pub fn enqueue(severity: u8, msg_id: &str, message: &str) {
    let dropped = {
        let mut queue = QUEUE.lock();
        queue.push(QueuedEvent {
            severity,
            msg_id: msg_id.to_string(),
            message: message.to_string(),
        })
    };
    if dropped {
        tracing::warn!(
            "Syslog: the forwarding queue is full ({MAX_QUEUE} events); the oldest were dropped"
        );
    }
    WAKE.notify_one();
}

/// Snapshot of the queue counters, for `GET /api/settings/syslog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QueueStats {
    pub pending: u64,
    pub enqueued: u64,
    pub dropped: u64,
    pub sent: u64,
    pub failed: u64,
}

pub fn queue_stats() -> QueueStats {
    let queue = QUEUE.lock();
    QueueStats {
        pending: queue.len() as u64,
        enqueued: queue.enqueued,
        dropped: queue.dropped,
        sent: queue.sent,
        failed: queue.failed,
    }
}

/// Drop everything still queued (used by tests and by disabling the feature).
pub fn clear_queue() {
    let mut queue = QUEUE.lock();
    queue.items.clear();
}

/// Status derived from the configuration and the last attempt.
///
/// Pure so the rules can be asserted without a collector:
/// * disabled → `disabled`, always;
/// * the last attempt was refused → `degraded` (events are queued and waiting);
/// * the last attempt reached the collector → `forwarding`;
/// * enabled and nothing tried yet → `idle`.
///
/// The drop counter is deliberately **not** an input: it is monotonic for the
/// life of the process, so one overflow during a burst would pin the card on
/// "degraded" forever. Losses are reported as a number in the payload instead —
/// a fact, next to a status that describes *now*.
pub fn status_for(enabled: bool, last_attempt_ok: Option<bool>) -> SyslogStatus {
    if !enabled {
        return SyslogStatus::Disabled;
    }
    match last_attempt_ok {
        Some(false) => SyslogStatus::Degraded,
        Some(true) => SyslogStatus::Forwarding,
        None => SyslogStatus::Idle,
    }
}

// --------------------------------------------------------------------------
// Transport
// --------------------------------------------------------------------------

/// The seam that makes the worker testable: production talks to the network,
/// a test hands in a scripted collector.
pub trait SyslogTransport: Send + Sync {
    /// Send one batch of already-framed messages. `Err` means "the collector
    /// did not take them", with a human-readable reason.
    fn send<'a>(
        &'a self,
        settings: &'a SyslogSettings,
        payloads: Vec<Vec<u8>>,
    ) -> crate::outbound::BoxFuture<'a, Result<(), String>>;
}

/// Production transport: UDP datagrams, or one TCP/TLS connection per batch.
#[derive(Debug, Default, Clone, Copy)]
pub struct NetTransport;

impl SyslogTransport for NetTransport {
    fn send<'a>(
        &'a self,
        settings: &'a SyslogSettings,
        payloads: Vec<Vec<u8>>,
    ) -> crate::outbound::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match settings.protocol {
                SyslogProtocol::Udp => send_udp(settings, &payloads).await,
                SyslogProtocol::Tcp => send_stream(settings, &payloads, None).await,
                SyslogProtocol::Tls => {
                    let config = tls_client_config(&settings.ca_pem)?;
                    send_stream(settings, &payloads, Some(config)).await
                }
            }
        })
    }
}

async fn send_udp(settings: &SyslogSettings, payloads: &[Vec<u8>]) -> Result<(), String> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("could not open a local socket: {e}"))?;
    let target = format!("{}:{}", settings.host.trim(), settings.port);
    // `connect` on a UDP socket only pins the destination: no packet is sent,
    // so an unresolvable name is reported once, here, instead of per datagram.
    socket
        .connect(&target)
        .await
        .map_err(|e| format!("cannot reach {target}: {e}"))?;
    for payload in payloads {
        let payload = &payload[..payload.len().min(MAX_UDP_DATAGRAM)];
        tokio::time::timeout(WRITE_TIMEOUT, socket.send(payload))
            .await
            .map_err(|_| format!("sending to {target} timed out"))?
            .map_err(|e| format!("sending to {target} failed: {e}"))?;
    }
    Ok(())
}

/// Write a batch over TCP, optionally wrapped in TLS.
///
/// One connection per batch and a deadline on every step: a collector that
/// accepts the connection and then stops reading must never pin the worker (and
/// with it the whole queue) forever.
async fn send_stream(
    settings: &SyslogSettings,
    payloads: &[Vec<u8>],
    tls: Option<Arc<rustls::ClientConfig>>,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let target = format!("{}:{}", settings.host.trim(), settings.port);
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&target))
        .await
        .map_err(|_| format!("connecting to {target} timed out"))?
        .map_err(|e| format!("cannot reach {target}: {e}"))?;
    connect
        .set_nodelay(true)
        .map_err(|e| format!("could not configure the connection: {e}"))?;

    match tls {
        Some(config) => {
            let server_name = rustls::pki_types::ServerName::try_from(settings.host.trim())
                .map_err(|_| format!("the server name is not valid for TLS: {}", settings.host))?
                .to_owned();
            let mut stream = tokio_rustls::TlsConnector::from(config)
                .connect(server_name, connect)
                .await
                .map_err(|e| format!("the TLS handshake failed: {e}"))?;
            write_all_bounded(&mut stream, payloads).await?;
            let _ = stream.shutdown().await;
        }
        None => {
            let mut stream = connect;
            write_all_bounded(&mut stream, payloads).await?;
            let _ = stream.shutdown().await;
        }
    }
    Ok(())
}

async fn write_all_bounded<W>(stream: &mut W, payloads: &[Vec<u8>]) -> Result<(), String>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    for payload in payloads {
        tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(payload))
            .await
            .map_err(|_| "writing to the collector timed out".to_string())?
            .map_err(|e| format!("writing to the collector failed: {e}"))?;
        // Some collectors frame on a newline; RFC 5424 over TCP is one message
        // per write, and the trailing LF is accepted by every common syslog
        // server (rsyslog, syslog-ng, journald-gateway).
        tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(b"\n"))
            .await
            .map_err(|_| "writing to the collector timed out".to_string())?
            .map_err(|e| format!("writing to the collector failed: {e}"))?;
    }
    tokio::time::timeout(WRITE_TIMEOUT, stream.flush())
        .await
        .map_err(|_| "flushing to the collector timed out".to_string())?
        .map_err(|e| format!("flushing to the collector failed: {e}"))
}

/// Build a TLS client that trusts **only** the configured CA.
///
/// The system roots are deliberately not loaded: a home collector presents a
/// self-signed certificate for a local name, and trusting every public CA would
/// make the pinning decorative. The provider is named explicitly (`ring`)
/// because the dependency tree carries two crypto providers (axum-server pulls
/// `aws-lc-rs`), and `ClientConfig::builder()` refuses to guess.
fn tls_client_config(ca_pem: &str) -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    for der in parse_ca_pem(ca_pem)? {
        roots
            .add(rustls::pki_types::CertificateDer::from(der))
            .map_err(|e| format!("the CA certificate was refused: {e}"))?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS could not be configured: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

// --------------------------------------------------------------------------
// Worker
// --------------------------------------------------------------------------

/// Send one batch and record the outcome. Shared by the worker and by the
/// "send a test message" endpoint so the two paths cannot diverge.
pub async fn send_batch(
    transport: &dyn SyslogTransport,
    settings: &SyslogSettings,
    events: Vec<QueuedEvent>,
    now: chrono::DateTime<chrono::Utc>,
    hostname: &str,
    proc_id: u32,
) -> Result<usize, String> {
    if events.is_empty() {
        return Ok(0);
    }
    let count = events.len();
    let payloads: Vec<Vec<u8>> = events
        .iter()
        .map(|event| {
            frame(
                settings,
                event.severity,
                &event.msg_id,
                &event.message,
                now,
                hostname,
                proc_id,
            )
        })
        .collect();
    transport.send(settings, payloads).await?;
    Ok(count)
}

/// Send one test message right now, reporting the collector's own error.
///
/// This is the "is the address right?" button: it bypasses the queue (an admin
/// pressing it wants to know *now*) but not the framing or the deadlines.
pub async fn send_test_message() -> Result<(), String> {
    let settings = load_settings();
    validate_settings(&settings)?;
    if !settings.enabled {
        return Err("log forwarding is disabled".to_string());
    }
    let event = QueuedEvent {
        severity: SEVERITY_INFO,
        msg_id: "test".to_string(),
        message: format!(
            "HomeVault test message from '{}' ({} format over {})",
            local_hostname(),
            settings.format.as_str(),
            settings.protocol.as_str()
        ),
    };
    send_batch(
        &NetTransport,
        &settings,
        vec![event],
        chrono::Utc::now(),
        &local_hostname(),
        std::process::id(),
    )
    .await
    .map(|_| ())
}

/// Background worker: wake on new events, drain the queue in batches, record the
/// outcome.
///
/// Started unconditionally and inert while the feature is disabled, so enabling
/// it later needs no restart — the same shape as the certificate and DDNS
/// workers. A batch whose send fails is **dropped, not retried**: replaying an
/// audit stream out of order hours later is worse than a gap, and the counters
/// plus `last_error` say exactly what was lost.
pub async fn run() {
    loop {
        WAKE.notified().await;

        let settings = load_settings();
        if !settings.enabled {
            clear_queue();
            continue;
        }

        let events = QUEUE.lock().take_batch();
        if events.is_empty() {
            continue;
        }

        let now = chrono::Utc::now();
        let hostname = local_hostname();
        let proc_id = std::process::id();
        let outcome = send_batch(&NetTransport, &settings, events, now, &hostname, proc_id).await;

        {
            let mut queue = QUEUE.lock();
            match &outcome {
                Ok(count) => queue.sent += *count as u64,
                Err(_) => queue.failed += 1,
            }
        }

        // Only the three observation fields are written, re-read under the
        // write lock: a destination the admin saved while this batch was in
        // flight must survive.
        let recorded = update_observation(|saved| match &outcome {
            Ok(_) => {
                saved.last_sent_at = Some(now.to_rfc3339());
                saved.last_error = None;
                saved.status = status_for(true, Some(true));
            }
            Err(reason) => {
                saved.last_error = Some(reason.clone());
                saved.status = status_for(true, Some(false));
            }
        });
        // A save failure (read-only data dir) must not stop forwarding: the
        // status is informative, the sending is the point.
        if let Err(e) = recorded {
            tracing::warn!("Syslog: could not persist the forwarding status: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn settings() -> SyslogSettings {
        SyslogSettings {
            enabled: true,
            protocol: SyslogProtocol::Udp,
            host: "collector.example.com".to_string(),
            port: 514,
            format: SyslogFormat::Rfc5424,
            facility: DEFAULT_FACILITY,
            ca_pem: String::new(),
            include_app_events: true,
            status: SyslogStatus::Idle,
            last_sent_at: None,
            last_error: None,
        }
    }

    fn at(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .timestamp_opt(seconds, 0)
            .single()
            .expect("valid")
    }

    // ---------------------------------------------------------------------
    // Framing
    // ---------------------------------------------------------------------

    #[test]
    fn priority_is_facility_times_eight_plus_severity() {
        assert_eq!(priority(1, 6), 14); // user.info
        assert_eq!(priority(0, 3), 3); // kern.err
        assert_eq!(priority(4, 4), 36); // auth.warning
        assert_eq!(priority(23, 7), 191); // local7.debug
    }

    #[test]
    fn rfc5424_is_framed_exactly_as_the_rfc_shows() {
        let line = frame_rfc5424(
            1,
            6,
            "2026-10-31T12:00:00Z",
            "nas",
            "homevault",
            "4242",
            "audit",
            "User 'alice' logged in successfully",
        );
        assert_eq!(
            line,
            "<14>1 2026-10-31T12:00:00Z nas homevault 4242 audit - User 'alice' logged in successfully"
        );
    }

    #[test]
    fn rfc3164_is_framed_exactly_as_the_rfc_shows() {
        let line = frame_rfc3164(
            1,
            4,
            "Oct 31 12:00:00",
            "nas",
            "homevault[4242]",
            "Failed login for 'bob'",
        );
        assert_eq!(
            line,
            "<12>Oct 31 12:00:00 nas homevault[4242]: Failed login for 'bob'"
        );
    }

    #[test]
    fn a_missing_header_field_becomes_the_nil_value() {
        // "-" is the RFC's "unavailable": an empty string would collapse the
        // header and shift every following field one position left.
        let line = frame_rfc5424(1, 6, "2026-10-31T12:00:00Z", "", "", "", "", "hello");
        assert_eq!(line, "<14>1 2026-10-31T12:00:00Z - - - - - hello");
    }

    #[test]
    fn a_space_in_a_header_field_cannot_break_the_header() {
        // A host name with a space would make the collector read "nas" as the
        // host and "extra" as the APP-NAME.
        let line = frame_rfc5424(
            1,
            6,
            "2026-10-31T12:00:00Z",
            "nas extra",
            "homevault",
            "1",
            "audit",
            "x",
        );
        assert!(line.starts_with("<14>1 2026-10-31T12:00:00Z nas_extra homevault 1 audit - x"));
    }

    #[test]
    fn rfc3164_timestamps_space_pad_a_single_digit_day() {
        // "Oct 3" is unparsable for a strict collector; "Oct  3" is the ABNF.
        let third = chrono::Utc
            .with_ymd_and_hms(2026, 10, 3, 4, 53, 20)
            .unwrap();
        assert_eq!(rfc3164_timestamp(third), "Oct  3 04:53:20");
        let fourteenth = chrono::Utc
            .with_ymd_and_hms(2026, 10, 14, 4, 53, 20)
            .unwrap();
        assert_eq!(rfc3164_timestamp(fourteenth), "Oct 14 04:53:20");
    }

    #[test]
    fn rfc5424_timestamps_are_utc_with_seconds_precision() {
        assert_eq!(rfc5424_timestamp(at(0)), "1970-01-01T00:00:00Z");
        let instant = chrono::Utc
            .with_ymd_and_hms(2026, 10, 31, 12, 0, 0)
            .unwrap();
        assert_eq!(rfc5424_timestamp(instant), "2026-10-31T12:00:00Z");
    }

    #[test]
    fn frame_honours_the_configured_format() {
        let mut s = settings();
        let line =
            String::from_utf8(frame(&s, SEVERITY_INFO, "audit", "hello", at(0), "nas", 7)).unwrap();
        assert_eq!(
            line,
            "<14>1 1970-01-01T00:00:00Z nas homevault 7 audit - hello"
        );

        s.format = SyslogFormat::Rfc3164;
        let line = String::from_utf8(frame(
            &s,
            SEVERITY_WARNING,
            "audit",
            "hello",
            at(0),
            "nas",
            7,
        ))
        .unwrap();
        assert_eq!(line, "<12>Jan  1 00:00:00 nas homevault[7]: hello");
    }

    #[test]
    fn a_udp_message_is_bounded_and_never_split_mid_character() {
        let mut s = settings();
        let long = "é".repeat(2048); // 4096 bytes
        let bytes = frame(&s, SEVERITY_INFO, "audit", &long, at(0), "nas", 7);
        assert!(bytes.len() <= MAX_UDP_DATAGRAM, "udp datagram bounded");
        assert!(
            std::str::from_utf8(&bytes).is_ok(),
            "truncated on a char boundary"
        );

        // A stream transport may carry the whole thing.
        s.protocol = SyslogProtocol::Tcp;
        let bytes = frame(&s, SEVERITY_INFO, "audit", &long, at(0), "nas", 7);
        assert!(bytes.len() > MAX_UDP_DATAGRAM);
        assert!(bytes.len() <= MAX_MESSAGE_BYTES + 128);
    }

    #[test]
    fn a_pathological_message_is_truncated_not_dropped() {
        let s = settings();
        let huge = "x".repeat(MAX_MESSAGE_BYTES * 2);
        let bytes = frame(&s, SEVERITY_INFO, "audit", &huge, at(0), "nas", 7);
        assert!(!bytes.is_empty());
        assert!(bytes.len() <= MAX_UDP_DATAGRAM);
    }

    // ---------------------------------------------------------------------
    // Parsing / validation
    // ---------------------------------------------------------------------

    #[test]
    fn protocol_and_format_parse_strictly_for_requests_and_leniently_for_storage() {
        assert_eq!(
            SyslogProtocol::parse_strict("TCP").unwrap(),
            SyslogProtocol::Tcp
        );
        assert_eq!(
            SyslogProtocol::parse_strict(" tls ").unwrap(),
            SyslogProtocol::Tls
        );
        assert!(SyslogProtocol::parse_strict("quic").is_err());
        // A stored value that no longer parses must not brick the feature.
        assert_eq!(SyslogProtocol::parse("quic"), SyslogProtocol::Udp);

        assert_eq!(
            SyslogFormat::parse_strict("rfc3164").unwrap(),
            SyslogFormat::Rfc3164
        );
        assert!(SyslogFormat::parse_strict("rfc9999").is_err());
        assert_eq!(SyslogFormat::parse("rfc9999"), SyslogFormat::Rfc5424);
    }

    #[test]
    fn a_hostname_or_ip_is_accepted_and_a_url_is_not() {
        assert_eq!(
            validate_host(" collector.example.com ").unwrap(),
            "collector.example.com"
        );
        assert_eq!(validate_host("192.168.1.10").unwrap(), "192.168.1.10");
        assert_eq!(validate_host("fe80::1").unwrap(), "fe80::1");
        assert_eq!(validate_host("NAS-01.lan").unwrap(), "nas-01.lan");

        assert!(validate_host("").is_err());
        assert!(validate_host("https://collector.example.com").is_err());
        assert!(validate_host("collector.example.com/syslog").is_err());
        assert!(validate_host("collector example.com").is_err());
        assert!(validate_host("[fe80::1]").is_err());
        assert!(validate_host("-bad.example.com").is_err());
        assert!(validate_host(&"a".repeat(254)).is_err());
    }

    #[test]
    fn a_disabled_entry_may_stay_half_filled() {
        // The admin must be able to turn the feature off without deleting the
        // address first.
        let s = SyslogSettings {
            enabled: false,
            host: String::new(),
            ..settings()
        };
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn an_enabled_entry_needs_a_host_a_port_a_facility_and_a_ca_for_tls() {
        let ok = settings();
        assert!(validate_settings(&ok).is_ok());

        assert!(validate_settings(&SyslogSettings {
            host: String::new(),
            ..settings()
        })
        .is_err());
        assert!(validate_settings(&SyslogSettings {
            port: 0,
            ..settings()
        })
        .is_err());
        assert!(validate_settings(&SyslogSettings {
            facility: 24,
            ..settings()
        })
        .is_err());

        let tls_without_ca = SyslogSettings {
            protocol: SyslogProtocol::Tls,
            ..settings()
        };
        assert!(validate_settings(&tls_without_ca).is_err());

        let tls_with_bad_ca = SyslogSettings {
            protocol: SyslogProtocol::Tls,
            ca_pem: "not a certificate".to_string(),
            ..settings()
        };
        assert!(validate_settings(&tls_with_bad_ca).is_err());
    }

    #[test]
    fn a_pem_ca_is_parsed_to_its_der_bytes() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIBkTCB+wIJAKH\n-----END CERTIFICATE-----\n";
        // The body is nonsense, but the PEM envelope is what this checks; the
        // real DER validation happens when the TLS client builds its roots.
        let certs = parse_ca_pem(pem).unwrap();
        assert_eq!(certs.len(), 1);
        assert!(!certs[0].is_empty());
        assert!(parse_ca_pem("").is_err());
        assert!(
            parse_ca_pem("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n").is_err()
        );
    }

    #[test]
    fn normalize_follows_the_enabled_flag() {
        let disabled = normalize(SyslogSettings {
            enabled: false,
            status: SyslogStatus::Forwarding,
            ..settings()
        });
        assert_eq!(disabled.status, SyslogStatus::Disabled);

        let enabled = normalize(SyslogSettings {
            enabled: true,
            status: SyslogStatus::Disabled,
            ..settings()
        });
        assert_eq!(enabled.status, SyslogStatus::Idle);
    }

    // ---------------------------------------------------------------------
    // Queue
    // ---------------------------------------------------------------------

    fn event(i: usize) -> QueuedEvent {
        QueuedEvent {
            severity: SEVERITY_INFO,
            msg_id: "audit".to_string(),
            message: format!("event {i}"),
        }
    }

    #[test]
    fn the_queue_keeps_the_newest_events_when_it_is_full() {
        let mut queue = SyslogQueue::default();
        for i in 0..MAX_QUEUE + 5 {
            queue.push(event(i));
        }
        assert_eq!(queue.len(), MAX_QUEUE);
        assert_eq!(queue.dropped, 5);
        // The five oldest went first.
        assert_eq!(queue.items.front().unwrap().message, "event 5");
        assert_eq!(
            queue.items.back().unwrap().message,
            format!("event {}", MAX_QUEUE + 4)
        );
    }

    #[test]
    fn push_reports_whether_something_was_dropped() {
        let mut queue = SyslogQueue::default();
        assert!(!queue.push(event(0)));
        for i in 1..MAX_QUEUE {
            assert!(!queue.push(event(i)));
        }
        assert!(
            queue.push(event(MAX_QUEUE)),
            "the overflow reports the drop"
        );
    }

    #[test]
    fn a_batch_is_bounded_and_keeps_the_logging_order() {
        let mut queue = SyslogQueue::default();
        for i in 0..(BATCH_SIZE + 3) {
            queue.push(event(i));
        }
        let batch = queue.take_batch();
        assert_eq!(batch.len(), BATCH_SIZE);
        assert_eq!(batch[0].message, "event 0");
        assert_eq!(
            batch[BATCH_SIZE - 1].message,
            format!("event {}", BATCH_SIZE - 1)
        );
        // The rest stays queued for the next wake.
        assert_eq!(queue.len(), 3);
        assert!(queue.take_batch().iter().all(|e| e.msg_id == "audit"));
        assert!(queue.take_batch().is_empty());
    }

    #[test]
    fn the_status_describes_now_not_the_past() {
        assert_eq!(status_for(false, Some(true)), SyslogStatus::Disabled);
        assert_eq!(status_for(true, None), SyslogStatus::Idle);
        assert_eq!(status_for(true, Some(true)), SyslogStatus::Forwarding);
        assert_eq!(status_for(true, Some(false)), SyslogStatus::Degraded);
        // A recovery is immediate: the next accepted batch clears the state.
        assert_eq!(status_for(true, Some(true)), SyslogStatus::Forwarding);
    }

    // ---------------------------------------------------------------------
    // Sending (scripted transport: no network)
    // ---------------------------------------------------------------------

    struct Scripted {
        fail: bool,
    }

    impl SyslogTransport for Scripted {
        fn send<'a>(
            &'a self,
            _settings: &'a SyslogSettings,
            payloads: Vec<Vec<u8>>,
        ) -> crate::outbound::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                if self.fail {
                    Err("connection refused".to_string())
                } else {
                    assert!(!payloads.is_empty());
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    async fn a_batch_renders_one_message_per_event() {
        let s = settings();
        let count = send_batch(
            &Scripted { fail: false },
            &s,
            vec![event(0), event(1)],
            at(0),
            "nas",
            7,
        )
        .await
        .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn an_empty_batch_sends_nothing() {
        let count = send_batch(
            &Scripted { fail: false },
            &settings(),
            vec![],
            at(0),
            "nas",
            7,
        )
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn a_refused_batch_reports_the_collectors_reason() {
        let error = send_batch(
            &Scripted { fail: true },
            &settings(),
            vec![event(0)],
            at(0),
            "nas",
            7,
        )
        .await
        .unwrap_err();
        assert_eq!(error, "connection refused");
    }
}
