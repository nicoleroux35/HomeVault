/* =============================================================================
 * File          : events.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Centralized helper for the user-facing, plain-English audit event log
 *   (logins, user/group/share changes, Docker actions, backup runs,
 *   security alerts). Written through the same tracing pipeline as the
 *   rest of the logs, and mirrored into the queryable `audit_log` table
 *   (P1-18) so the System Journal can filter it by actor/action/severity
 *   and an admin can export it.
 *
 * Dependencies:
 *   - tracing (>= 0.1)
 *
 * Usage:
 *   `crate::events::log_event(...)` / `log_event_warn(...)` from handlers
 *   on every auditable mutation.
 * =============================================================================
 */

//! Centralized "event log" helper for user-facing, plain-English audit
//! events (login, user/group/share changes, Docker actions, backup runs,
//! security alerts, etc.).
//!
//! These are written through the same `tracing` pipeline as every other
//! log line in this codebase, so they flow into the System Journal app's
//! "app" source (see `src/handlers/journal.rs`) with no new infrastructure
//! — the only thing that sets them apart is a fixed `target` (so they're
//! easy to grep for) and a convention: always a complete, plain-English
//! sentence naming the actor and the thing that happened, never a raw
//! struct dump or a terse technical fragment. Since P1-18 the same sentence
//! is also queued for the `audit_log` table (see `src/audit.rs`); that push
//! is non-blocking and never changes the caller's behaviour.
//!
//! Always English, regardless of the active UI language — this matches
//! the project-wide precedent of leaving log/check-style content
//! untranslated (see CLAUDE.md's Health/Downloader/Syncthing sections);
//! only this page's own UI chrome is localized.
//!
//! Callers should read as a sentence, e.g.:
//! `log_event(format!("User '{username}' logged in successfully"))`.

/// Target used for every event logged through this module, so they can be
/// filtered/greped independently of routine internal `tracing` calls.
pub const EVENT_TARGET: &str = "homevault::event";

/// Record a plain-English application event at info level (the normal
/// case: something happened successfully).
pub fn log_event(message: impl std::fmt::Display) {
    let message = message.to_string();
    tracing::info!(target: EVENT_TARGET, "{}", message);
    forward(&message, crate::syslog_forward::SEVERITY_INFO);
    // P1-18: also persist the sentence in the queryable audit trail. The call
    // is a non-blocking channel push; it never delays the mutation that logged
    // it and is a no-op before `audit::init` runs.
    crate::audit::record_event(&message, crate::audit::Severity::Info);
}

/// Record a plain-English application event at warn level — failed logins,
/// detected threats, degraded arrays, and other events that succeeded as
/// an *operation* (nothing crashed) but represent something an admin
/// should notice.
pub fn log_event_warn(message: impl std::fmt::Display) {
    let message = message.to_string();
    tracing::warn!(target: EVENT_TARGET, "{}", message);
    forward(&message, crate::syslog_forward::SEVERITY_WARNING);
    crate::audit::record_event(&message, crate::audit::Severity::Warning);
}

/// Authentication event with the structured fields an investigator needs:
/// the account, the action (`auth.login`, `auth.login.failed`,
/// `auth.logout`, …) and the client address. The plain-English sentence is
/// still the canonical message; this only adds the columns.
pub fn log_auth_event(
    message: impl std::fmt::Display,
    warning: bool,
    actor: &str,
    action: &str,
    client_ip: &str,
) {
    let message = message.to_string();
    if warning {
        tracing::warn!(target: EVENT_TARGET, "{}", message);
        forward(&message, crate::syslog_forward::SEVERITY_WARNING);
    } else {
        tracing::info!(target: EVENT_TARGET, "{}", message);
        forward(&message, crate::syslog_forward::SEVERITY_INFO);
    }
    crate::audit::record(crate::audit::AuditRecord {
        severity: if warning {
            crate::audit::Severity::Warning
        } else {
            crate::audit::Severity::Info
        },
        actor: Some(actor.to_string()),
        action: Some(action.to_string()),
        target: Some(actor.to_string()),
        client_ip: Some(client_ip.to_string()),
        message,
    });
}

/// Hand the event to the remote-syslog forwarder (QUICK-WIN QW-08).
///
/// One call site for both levels, so a new event function cannot ship without
/// being forwarded. The call is a queue push: it never touches the network, so
/// a dead collector can never slow down or fail the mutation that logged it.
/// While forwarding is off it is a cheap settings read and nothing else.
fn forward(message: &str, severity: u8) {
    if !crate::syslog_forward::forwarding_enabled() {
        return;
    }
    crate::syslog_forward::enqueue(severity, "audit", message);
}
