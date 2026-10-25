/* =============================================================================
 * File          : mdns.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   mDNS/Zeroconf publishing through Avahi: makes the appliance reachable
 *   at homevault.local and advertises the web UI plus every share service
 *   that currently has an enabled share, so they surface in file-manager
 *   network views. A background reconciler keeps the published set in sync
 *   with the actual service state — nothing to configure by hand.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup; handlers/mdns.rs exposes the read-only
 *   status (see docs/guides/mdns.md).
 * =============================================================================
 */

//! mDNS / Zeroconf publishing via **Avahi** — makes HomeVault reachable at
//! `homevault.local` on the LAN and advertises the web UI plus whichever
//! share services (SMB / NFS / FTP / SFTP / WebDAV) currently have at least
//! one enabled share, so they surface in Finder / file-manager "Network"
//! views and in generic mDNS browsers.
//!
//! Scope & privacy: this is **link-local only** (multicast 224.0.0.251:5353 /
//! ff02::fb) — no outbound traffic, no third party. It is the same default
//! every mainstream NAS distribution ships (publishing `<host>.local` via
//! Avahi/Bonjour out of the box). It advertises
//! only service *names + ports* that HomeVault already listens on; it never
//! carries credentials or personal data. An admin who wants it off stops
//! `avahi-daemon` (`systemctl disable --now avahi-daemon`) or deletes
//! [`SERVICE_FILE`] — see `PRIVACY.md`.
//!
//! Predictable name: we pin the published host to `homevault` via Avahi's
//! `host-name=` override in [`AVAHI_CONF`], rather than following whatever
//! the admin named the OS host, so the address is always `homevault.local`.
//! Avahi's built-in conflict resolution renames a second HomeVault box on the
//! same segment to `homevault-2.local`.
//!
//! All writes go through `sudo tee` — the process may not be root and a bare
//! `std::fs::write` to `/etc/avahi/...` would silently `EACCES` (the same
//! class of bug already fixed in `network.rs` / `shares.rs`).

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;

use crate::shares::{ShareType, SharesService};
use crate::utils::process::{run_timed, run_timed_with_stdin};

/// The default name HomeVault publishes itself as — the `.local` address is
/// `<hostname>.local`, independent of the OS hostname.
pub const MDNS_HOSTNAME: &str = "homevault";

/// The effective mDNS hostname for this instance.
///
/// `HV_MDNS_HOSTNAME` overrides the default so several HomeVault instances can
/// coexist on one LAN during development (`hv-dev-1.local`, `hv-dev-2.local`,
/// …) — see `scripts/dev-instance.sh`. Production leaves it unset and gets
/// `homevault.local`. An override with characters Avahi would reject is
/// ignored rather than written into the config.
pub fn mdns_hostname() -> String {
    std::env::var("HV_MDNS_HOSTNAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 63
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or_else(|| MDNS_HOSTNAME.to_string())
}
/// Avahi's main config file — we only ever touch its `host-name=` key.
pub const AVAHI_CONF: &str = "/etc/avahi/avahi-daemon.conf";
/// Environment override for [`AVAHI_CONF`].
///
/// Same seam as `RCLONE_CONFIG` (`handlers/rclone.rs`): it exists so a test can
/// point the whole publish/unpublish path at a temp directory and prove what
/// lands on disk **without writing to `/etc`**. Unset in production, where the
/// compiled-in path is the only correct one.
pub const AVAHI_CONF_ENV: &str = "HV_AVAHI_CONF";
/// Environment override for [`SERVICE_FILE`] — see [`AVAHI_CONF_ENV`].
pub const SERVICE_FILE_ENV: &str = "HV_AVAHI_SERVICE_FILE";
/// The DNS-SD service definition HomeVault owns. Avahi loads every
/// `*.service` in this directory; this is the only one we write.
pub const SERVICE_FILE: &str = "/etc/avahi/services/homevault.service";

/// The `avahi-daemon.conf` this process edits: [`AVAHI_CONF_ENV`] when set and
/// non-empty, [`AVAHI_CONF`] otherwise.
pub fn avahi_conf_path() -> String {
    env_path(AVAHI_CONF_ENV).unwrap_or_else(|| AVAHI_CONF.to_string())
}

/// The DNS-SD service file this process owns: [`SERVICE_FILE_ENV`] when set and
/// non-empty, [`SERVICE_FILE`] otherwise.
pub fn service_file_path() -> String {
    env_path(SERVICE_FILE_ENV).unwrap_or_else(|| SERVICE_FILE.to_string())
}

/// A non-empty environment override, or `None`. An exported-but-empty value is
/// ignored rather than resolving to `""` (which would make `tee` fail with a
/// confusing "No such file").
fn env_path(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// How often the background reconciler re-derives the advertised set (which
/// shares are enabled can change from the Shares page at any time) and
/// rewrites [`SERVICE_FILE`] if — and only if — the content changed.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(120);

/// Deadline on a `sudo tee`/`sudo rm` write to `/etc/avahi`. A `sudo` prompt
/// waiting on a terminal that will never answer must not pin the reconciler
/// thread for ever (ROADMAP X.2).
const MDNS_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline on a `systemctl`/`busctl` call against Avahi. A real service
/// restart can wait on its own dependencies, hence the larger ceiling than
/// the probe below.
const MDNS_SERVICE_TIMEOUT: Duration = Duration::from_secs(60);
/// Deadline on a read-only probe (`which`, `systemctl is-active`, `busctl
/// get-property`). Each one is a few milliseconds on a healthy host.
const MDNS_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Root-write helpers (mirrors network.rs::write_as_root)
// ---------------------------------------------------------------------------

fn sudo_tee(path: &str, content: &str) -> Result<(), String> {
    // `run_timed_with_stdin` feeds the body from its own thread and closes the
    // pipe, so a `sudo` that never reads cannot deadlock the caller.
    let mut cmd = Command::new("sudo");
    cmd.args(["tee", path]);
    let out = run_timed_with_stdin(&mut cmd, MDNS_WRITE_TIMEOUT, content.as_bytes())?;
    if !out.status.success() {
        return Err(format!(
            "failed to write {}: {}",
            path,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn sudo_rm(path: &str) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["rm", "-f", path]);
    let out = run_timed(&mut cmd, MDNS_WRITE_TIMEOUT)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn systemctl(action: &str) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["systemctl", action, "avahi-daemon"]);
    let out = run_timed(&mut cmd, MDNS_SERVICE_TIMEOUT)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Whether a systemd unit is currently active. Read-only, needs no root
/// (same query `dependencies.rs` uses).
fn unit_active(unit: &str) -> bool {
    let mut cmd = Command::new("systemctl");
    cmd.args(["is-active", "--quiet", unit]);
    run_timed(&mut cmd, MDNS_PROBE_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `avahi-daemon` binary present on this host (matches `dependencies.rs`).
pub fn avahi_available() -> bool {
    let mut cmd = Command::new("which");
    cmd.arg("avahi-daemon");
    run_timed(&mut cmd, MDNS_PROBE_TIMEOUT)
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
        || std::path::Path::new("/usr/sbin/avahi-daemon").exists()
}

// ---------------------------------------------------------------------------
// Pure builders (unit-tested)
// ---------------------------------------------------------------------------

/// One advertised DNS-SD service: its type, port, and any TXT records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedService {
    pub svc_type: &'static str,
    pub port: u16,
    pub txt: Vec<String>,
}

/// Ensure `host-name=<host_name>` is set in the `[server]` section of an
/// `avahi-daemon.conf` body. Returns `Some(new_body)` when a change is
/// needed, `None` when the file already has exactly that value. Every other
/// line (comments, other keys, other sections) is preserved verbatim — the
/// same "targeted edit, don't clobber" rule as `network.rs::build_resolv_conf`.
pub fn ensure_host_name(existing: &str, host_name: &str) -> Option<String> {
    let want = format!("host-name={host_name}");

    // Classify every line: is it inside `[server]`, and is it a `host-name`
    // key line (commented or not)?
    #[derive(PartialEq)]
    enum Kind {
        Other,
        ServerHeader,
        HostNameInServer,
    }
    let mut in_server = false;
    let mut classified: Vec<(Kind, &str)> = Vec::new();
    let mut correct_key_lines = 0usize;
    let mut stale_key_lines = 0usize;
    let mut has_server = false;

    for line in existing.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_server = t.eq_ignore_ascii_case("[server]");
            has_server |= in_server;
            classified.push((
                if in_server {
                    Kind::ServerHeader
                } else {
                    Kind::Other
                },
                line,
            ));
            continue;
        }
        let key = t.trim_start_matches('#').trim_start();
        let is_host_name = key.split('=').next().map(str::trim) == Some("host-name");
        if in_server && is_host_name {
            if t == want {
                correct_key_lines += 1;
            } else {
                stale_key_lines += 1;
            }
            classified.push((Kind::HostNameInServer, line));
        } else {
            classified.push((Kind::Other, line));
        }
    }

    // Nothing to do: exactly one correct line, no stale/commented ones.
    if has_server && correct_key_lines == 1 && stale_key_lines == 0 {
        return None;
    }

    // Rebuild: drop every host-name line in [server], then re-insert `want`
    // right after the `[server]` header (creating the section if needed).
    let mut out = String::with_capacity(existing.len() + want.len() + 16);
    let mut inserted = false;
    for (kind, line) in &classified {
        match kind {
            Kind::HostNameInServer => continue, // dropped; re-added below
            Kind::ServerHeader => {
                out.push_str(line);
                out.push('\n');
                out.push_str(&want);
                out.push('\n');
                inserted = true;
            }
            Kind::Other => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !inserted {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("[server]\n");
        out.push_str(&want);
        out.push('\n');
    }
    Some(out)
}

/// Drop every `host-name` key line from the `[server]` section of an
/// `avahi-daemon.conf` body, leaving the rest byte-for-byte intact.
///
/// Returns `Some(new_body)` when there was something to remove, `None` when the
/// file carries no override (so the caller can skip the `sudo tee`). Both the
/// active key and a commented-out one are dropped: leaving `#host-name=…`
/// behind would make the file read as if HomeVault still owned the name, and a
/// later `ensure_host_name` would have two lines to reason about.
///
/// Pure, so the "discovery off really removes the name" half is unit-testable
/// without an Avahi daemon — the only host-touching part left is the `sudo
/// tee` itself (ROADMAP 29.1).
pub fn remove_host_name(existing: &str) -> Option<String> {
    let mut out = String::with_capacity(existing.len());
    let mut in_server = false;
    let mut removed = false;

    for line in existing.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_server = t.eq_ignore_ascii_case("[server]");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let key = t.trim_start_matches('#').trim_start();
        let is_host_name = key.split('=').next().map(str::trim) == Some("host-name");
        if in_server && is_host_name {
            removed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    if removed {
        Some(out)
    } else {
        None
    }
}

/// Render the `/etc/avahi/services/homevault.service` XML for the given web
/// port and advertised service set. Pure — no I/O.
pub fn render_service_file(web_port: u16, services: &[AdvertisedService]) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" standalone='no'?>\n");
    xml.push_str("<!DOCTYPE service-group SYSTEM \"avahi-service.dtd\">\n");
    xml.push_str("<!-- Generated by HomeVault (src/mdns.rs) — do not edit by hand. -->\n");
    xml.push_str("<service-group>\n");
    xml.push_str("  <name replace-wildcards=\"yes\">HomeVault on %h</name>\n");

    // The web UI is always advertised (that's the whole point — the address).
    xml.push_str("  <service>\n");
    xml.push_str("    <type>_http._tcp</type>\n");
    xml.push_str(&format!("    <port>{web_port}</port>\n"));
    xml.push_str("    <txt-record>path=/</txt-record>\n");
    xml.push_str("  </service>\n");

    for svc in services {
        xml.push_str("  <service>\n");
        xml.push_str(&format!("    <type>{}</type>\n", svc.svc_type));
        xml.push_str(&format!("    <port>{}</port>\n", svc.port));
        for t in &svc.txt {
            xml.push_str(&format!("    <txt-record>{}</txt-record>\n", xml_escape(t)));
        }
        xml.push_str("  </service>\n");
    }

    xml.push_str("</service-group>\n");
    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Live detection
// ---------------------------------------------------------------------------

/// Count enabled shares per protocol, best-effort (a read error ⇒ "none").
fn enabled_share_types() -> Vec<ShareType> {
    let shares = SharesService::get_shares().unwrap_or_default();
    let mut out = Vec::new();
    for st in [
        ShareType::Smb,
        ShareType::Nfs,
        ShareType::Ftp,
        ShareType::Webdav,
        ShareType::Sftp,
    ] {
        if shares.iter().any(|s| s.share_type == st && s.enabled) {
            out.push(st);
        }
    }
    out
}

/// Build the advertised-service list from the current host state: a protocol
/// is advertised only when it has an enabled share **and** its daemon is
/// actually running (advertising a dead port just makes clients time out).
pub fn detect_services() -> Vec<AdvertisedService> {
    let mut out = Vec::new();
    for st in enabled_share_types() {
        match st {
            ShareType::Smb if unit_active("smbd") => out.push(AdvertisedService {
                svc_type: "_smb._tcp",
                port: 445,
                txt: vec![],
            }),
            ShareType::Nfs if unit_active("nfs-server") => out.push(AdvertisedService {
                svc_type: "_nfs._tcp",
                port: 2049,
                txt: vec![],
            }),
            ShareType::Ftp if unit_active("vsftpd") => {
                let port = SharesService::get_ftp_config()
                    .map(|c| c.port)
                    .unwrap_or(21);
                out.push(AdvertisedService {
                    svc_type: "_ftp._tcp",
                    port,
                    txt: vec![],
                });
            }
            ShareType::Webdav if crate::webdav::running_port().is_some() => {
                let port = crate::webdav::running_port().unwrap_or(8082);
                out.push(AdvertisedService {
                    svc_type: "_webdav._tcp",
                    port,
                    txt: vec!["path=/".into()],
                });
            }
            ShareType::Sftp if unit_active("homevault-sftp") => {
                let port = SharesService::get_sftp_config()
                    .map(|c| c.port)
                    .unwrap_or(2222);
                out.push(AdvertisedService {
                    svc_type: "_sftp-ssh._tcp",
                    port,
                    txt: vec![],
                });
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Publish / unpublish
// ---------------------------------------------------------------------------

/// Whether the appliance advertises itself over mDNS at all.
///
/// A plain `AtomicBool` rather than a read of `GENERAL_SETTINGS` on every tick:
/// this module is the *low* layer, so it must not depend on `handlers::settings`
/// (which itself imports `crate::mdns` for the status/TLS names). The settings
/// path is the writer — `handlers::settings::handle_general_settings_update`
/// and `settings_persistence::init_persistence`, both through
/// [`set_enabled`]. Default `true`: discovery is on unless an admin turned it
/// off, matching the shipped default of the setting.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Whether the reconciler may advertise, and whether an admin has opted out.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Set the flag without touching the host (used at boot and by the settings
/// save when the value is unchanged). Returns `true` when the value moved.
pub fn set_enabled(enabled: bool) -> bool {
    ENABLED.swap(enabled, Ordering::Relaxed) != enabled
}

/// Apply an admin's mDNS choice to the host: publish when it is switched on,
/// remove both artefacts when it is switched off.
///
/// Order matters. Turning it **on** writes the config/service files and
/// (re)applies the host name *before* the flag flips, so the daemon is already
/// advertising when the next tick sees `enabled = true`. Turning it **off**
/// unpublishes first — the service file and the `host-name` override both
/// leave `/etc/avahi`, which is what the old code never did: `unpublish` was
/// dead code, so switching discovery off left HomeVault advertised on the LAN
/// and the appliance's name pinned in Avahi's config (ROADMAP 29.1).
pub fn apply_enabled(web_port: u16, enabled: bool) -> Result<(), String> {
    // Deliberately **not** gated on a transition: both halves are idempotent
    // (`publish` only writes what changed, `unpublish` only removes what is
    // there), and re-applying is what repairs a host left half-configured by a
    // hand-edited `settings.json` or an interrupted save.
    if enabled {
        publish(web_port)?;
        set_enabled(true);
        Ok(())
    } else {
        unpublish()?;
        set_enabled(false);
        Ok(())
    }
}

/// (Re)publish HomeVault over mDNS: pin the `homevault` host-name and write a
/// service file describing the web UI + currently-running shares. Idempotent
/// — only touches `avahi-daemon.conf` / the service file when their content
/// actually changed, and only reloads (or restarts, if the host-name key
/// moved — a plain reload does not re-read it) when something was written.
///
/// Best-effort: returns `Ok(())` and does nothing when `avahi-daemon` isn't
/// installed (the startup dependency check already warns about that).
pub fn publish(web_port: u16) -> Result<(), String> {
    if !avahi_available() {
        return Ok(());
    }

    let mut needs_restart = false;
    let mut needs_reload = false;

    // 1. Pin the published name.
    //
    // The `host-name=` key is read by `avahi-daemon` **at startup only**
    // (`avahi-daemon --reload` re-reads `/etc/avahi/services/*`, not this
    // file — see avahi-daemon(8)), so the old code restarted the daemon on
    // every change. That restart drops *every* other advertisement on the
    // host for a few seconds: the admin's printers, other appliances' shares,
    // anything else Avahi publishes on that segment. Avahi exposes
    // `SetHostName` on its D-Bus interface, which renames the running daemon
    // without a restart, so that is what we use — the config file is still
    // written for the next boot, and a **restart remains the fallback** for a
    // host whose D-Bus call is refused (no `busctl`, no system bus, Avahi not
    // on the bus). ROADMAP 29.2.
    let conf_path = avahi_conf_path();
    let conf = std::fs::read_to_string(&conf_path).unwrap_or_default();
    let mut rename_now = false;
    if let Some(new_conf) = ensure_host_name(&conf, &mdns_hostname()) {
        sudo_tee(&conf_path, &new_conf)?;
        rename_now = true;
    }

    // 2. Describe the services.
    let desired = render_service_file(web_port, &detect_services());
    let service_path = service_file_path();
    let current = std::fs::read_to_string(&service_path).unwrap_or_default();
    if current != desired {
        sudo_tee(&service_path, &desired)?;
        needs_reload = true;
    }

    // 3. Apply.
    if rename_now {
        if let Err(e) = set_running_host_name(&mdns_hostname()) {
            tracing::warn!(
                "mDNS: live rename to '{}' failed ({e}); falling back to an \
                 avahi-daemon restart",
                mdns_hostname()
            );
            needs_restart = true;
        } else {
            // The rename is live; the service file still needs a reload.
            needs_restart = false;
        }
    }
    if needs_restart {
        systemctl("restart")?;
    } else if needs_reload {
        // `avahi-daemon --reload` picks up service-file changes without a
        // full restart (it re-reads `/etc/avahi/services/*` — see
        // avahi-daemon(8)); fall back to `systemctl reload` if the binary call
        // isn't permitted. Both are bounded like every other call here.
        let mut direct_cmd = Command::new("sudo");
        direct_cmd.args(["avahi-daemon", "--reload"]);
        let direct = run_timed(&mut direct_cmd, MDNS_SERVICE_TIMEOUT)
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !direct {
            systemctl("reload")?;
        }
    }
    Ok(())
}

/// Remove HomeVault's mDNS advertisement: delete the DNS-SD service file
/// **and** the `host-name` override Avahi was pinned to, then reload the
/// daemon so the removal is live.
///
/// Both artefacts have to go. An earlier version kept the `host-name` override
/// on the grounds that it was "harmless on its own" — but it is not: the
/// appliance keeps answering as `homevault.local` (and keeps a name reserved
/// on the segment) after the admin switched discovery off, which is exactly
/// the privacy default `PRIVACY.md` promises. `unpublish` itself had no caller
/// at all, so neither artefact was ever removed (ROADMAP 29.1).
///
/// A reload — not a restart — is the right signal here: it re-reads
/// `/etc/avahi/services/*`, which is the file we deleted, and it does not drop
/// the other advertisements on the host.
pub fn unpublish() -> Result<(), String> {
    if !avahi_available() {
        return Ok(());
    }
    let mut changed = false;

    let service_path = service_file_path();
    if std::path::Path::new(&service_path).exists() {
        sudo_rm(&service_path)?;
        changed = true;
    }

    // Drop the `host-name=` override so the daemon falls back to the host's own
    // name. Note the *live* name can only follow with a restart (Avahi reads
    // this key at startup), which is the honest cost of turning discovery off
    // and is why the loop is stopped first.
    let conf_path = avahi_conf_path();
    let conf = std::fs::read_to_string(&conf_path).unwrap_or_default();
    if let Some(stripped) = remove_host_name(&conf) {
        sudo_tee(&conf_path, &stripped)?;
        changed = true;
        // Live rename back to the host's own name, when D-Bus answers.
        let host = hostname::get()
            .map(|h| h.to_string_lossy().to_lowercase())
            .unwrap_or_else(|_| "localhost".to_string());
        if let Err(e) = set_running_host_name(&host) {
            tracing::debug!("mDNS: live rename back to '{host}' skipped ({e})");
            let _ = systemctl("restart");
        }
    }

    if changed {
        let _ = systemctl("reload");
    }
    Ok(())
}

/// Rename the **running** `avahi-daemon` through its D-Bus interface, so a
/// host-name change does not restart the daemon and drop every other
/// advertisement on the host (ROADMAP 29.2).
///
/// `sudo` because `SetHostName` is a privileged method on the system bus.
/// Verified by reading the property back: a call that returns 0 but leaves the
/// old name in place (a bus that swallowed the request) must not be reported
/// as success, or the fallback restart never happens.
fn set_running_host_name(name: &str) -> Result<(), String> {
    let mut call = Command::new("sudo");
    call.args([
        "busctl",
        "call",
        "org.freedesktop.Avahi",
        "/",
        "org.freedesktop.Avahi.Server",
        "SetHostName",
        "s",
        name,
    ]);
    let out = run_timed(&mut call, MDNS_SERVICE_TIMEOUT)?;
    if !out.status.success() {
        return Err(format!(
            "busctl SetHostName failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let mut read = Command::new("busctl");
    read.args([
        "get-property",
        "org.freedesktop.Avahi",
        "/",
        "org.freedesktop.Avahi.Server",
        "org.freedesktop.Avahi.Server",
        "HostName",
    ]);
    let out = run_timed(&mut read, MDNS_PROBE_TIMEOUT)?;
    let reported = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && !reported.contains(name) {
        return Err(format!(
            "busctl reported '{}' after SetHostName({name})",
            reported.trim()
        ));
    }
    Ok(())
}

/// Background reconciler: publishes once immediately, then every
/// [`RECONCILE_INTERVAL`] re-derives the advertised set and rewrites the
/// service file if it changed (a share was enabled/disabled, a daemon came
/// up or went down). Cheap when nothing changed — a few `systemctl
/// is-active` calls and a string compare.
pub async fn run_publisher_loop(web_port: u16) {
    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
    loop {
        interval.tick().await;
        // Switched off in Settings: the service file and the name override were
        // removed by `apply_enabled`, and re-publishing here would put them
        // straight back on the next tick (ROADMAP 29.1).
        if !is_enabled() {
            continue;
        }
        let res = tokio::task::spawn_blocking(move || publish(web_port)).await;
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("mDNS publish failed: {e}"),
            Err(e) => tracing::warn!("mDNS publish task panicked: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Status (for the API / diagnostics)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MdnsStatus {
    /// The admin's choice: `false` when discovery is switched off in Settings.
    pub enabled: bool,
    /// `avahi-daemon` installed on this host.
    pub available: bool,
    /// `avahi-daemon` currently running.
    pub running: bool,
    /// The published short name (always `homevault` unless Avahi had to
    /// resolve a conflict).
    pub host_name: String,
    /// The full mDNS address, e.g. `homevault.local`.
    pub fqdn: String,
    /// Ready-to-click URL for the web UI over mDNS.
    pub http_url: String,
    /// DNS-SD service types currently advertised (web UI + running shares).
    pub services: Vec<String>,
}

pub fn status(web_port: u16) -> MdnsStatus {
    let available = avahi_available();
    let running = available && unit_active("avahi-daemon");
    let fqdn = format!("{}.local", mdns_hostname());
    let mut services = vec!["_http._tcp".to_string()];
    services.extend(detect_services().iter().map(|s| s.svc_type.to_string()));
    MdnsStatus {
        enabled: is_enabled(),
        available,
        running,
        host_name: mdns_hostname(),
        fqdn: fqdn.clone(),
        http_url: format!("http://{fqdn}:{web_port}"),
        services,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_host_name_inserts_into_existing_server_section() {
        let conf = "[server]\nuse-ipv4=yes\nuse-ipv6=yes\n\n[publish]\npublish-hinfo=no\n";
        let out = ensure_host_name(conf, "homevault").expect("should change");
        assert!(out.contains("host-name=homevault"));
        // the host-name line must sit inside [server], before [publish]
        let hn = out.find("host-name=homevault").unwrap();
        let pub_sec = out.find("[publish]").unwrap();
        assert!(hn < pub_sec);
        // other keys preserved
        assert!(out.contains("use-ipv4=yes"));
        assert!(out.contains("publish-hinfo=no"));
    }

    #[test]
    fn test_ensure_host_name_replaces_wrong_or_commented_value() {
        let conf = "[server]\n#host-name=foo\nhost-name=nas\nuse-ipv4=yes\n";
        let out = ensure_host_name(conf, "homevault").expect("should change");
        assert!(out.contains("host-name=homevault"));
        assert!(!out.contains("host-name=nas"));
        // the stale commented line is dropped, not left to confuse
        assert_eq!(out.matches("host-name=").count(), 1);
    }

    #[test]
    fn test_ensure_host_name_noop_when_already_correct() {
        let conf = "[server]\nhost-name=homevault\nuse-ipv4=yes\n";
        assert!(ensure_host_name(conf, "homevault").is_none());
    }

    #[test]
    fn test_ensure_host_name_appends_server_section_when_missing() {
        let conf = "[publish]\npublish-workstation=no\n";
        let out = ensure_host_name(conf, "homevault").expect("should change");
        assert!(out.contains("[server]"));
        assert!(out.contains("host-name=homevault"));
        assert!(out.contains("publish-workstation=no"));
    }

    #[test]
    fn test_ensure_host_name_handles_empty_file() {
        let out = ensure_host_name("", "homevault").expect("should change");
        assert!(out.contains("[server]"));
        assert!(out.contains("host-name=homevault"));
    }

    #[test]
    fn test_render_service_file_always_has_web_ui() {
        let xml = render_service_file(8080, &[]);
        assert!(xml.contains("<type>_http._tcp</type>"));
        assert!(xml.contains("<port>8080</port>"));
        assert!(xml.trim_start().starts_with("<?xml"));
        assert!(xml.contains("replace-wildcards=\"yes\""));
        // exactly one service block when no shares
        assert_eq!(xml.matches("<service>").count(), 1);
    }

    #[test]
    fn test_render_service_file_includes_shares_and_ports() {
        let svcs = vec![
            AdvertisedService {
                svc_type: "_smb._tcp",
                port: 445,
                txt: vec![],
            },
            AdvertisedService {
                svc_type: "_sftp-ssh._tcp",
                port: 2222,
                txt: vec![],
            },
            AdvertisedService {
                svc_type: "_webdav._tcp",
                port: 8082,
                txt: vec!["path=/".into()],
            },
        ];
        let xml = render_service_file(9000, &svcs);
        assert!(xml.contains("<type>_smb._tcp</type>"));
        assert!(xml.contains("<port>2222</port>"));
        assert!(xml.contains("<txt-record>path=/</txt-record>"));
        assert_eq!(xml.matches("<service>").count(), 4); // http + 3
    }

    #[test]
    fn test_render_service_file_escapes_txt_records() {
        let svcs = vec![AdvertisedService {
            svc_type: "_smb._tcp",
            port: 445,
            txt: vec!["name=A & B <x>".into()],
        }];
        let xml = render_service_file(80, &svcs);
        assert!(xml.contains("A &amp; B &lt;x&gt;"));
        assert!(!xml.contains("A & B <x>"));
    }

    #[test]
    fn test_mdns_hostname_defaults_without_override() {
        // No `HV_MDNS_HOSTNAME` in the test process → the canonical name.
        assert_eq!(mdns_hostname(), MDNS_HOSTNAME);
    }

    // -----------------------------------------------------------------
    // ROADMAP 29.1 — turning discovery off really removes both artefacts
    // -----------------------------------------------------------------

    /// The assertion 29.1 needed: `unpublish` used to leave the `host-name`
    /// override in place, so switching discovery off kept the appliance
    /// answering as `homevault.local`.
    #[test]
    fn remove_host_name_drops_the_override_and_keeps_every_other_line() {
        let conf = "[server]\nuse-ipv4=yes\nhost-name=homevault\nuse-ipv6=no\n\n[publish]\npublish-hinfo=no\n";
        let out = remove_host_name(conf).expect("there is an override to remove");
        assert!(!out.contains("host-name"), "{out}");
        // Nothing else moved.
        assert!(out.contains("use-ipv4=yes"));
        assert!(out.contains("use-ipv6=no"));
        assert!(out.contains("[publish]"));
        assert!(out.contains("publish-hinfo=no"));
    }

    #[test]
    fn remove_host_name_drops_a_commented_override_too() {
        // A `#host-name=` line reads as "this file still owns the name", and it
        // would give a later `ensure_host_name` two lines to reason about.
        let conf = "[server]\n#host-name=old\nhost-name=homevault\n";
        let out = remove_host_name(conf).expect("should remove");
        assert_eq!(out.matches("host-name").count(), 0, "{out}");
    }

    #[test]
    fn remove_host_name_is_a_noop_without_an_override() {
        assert!(remove_host_name("[server]\nuse-ipv4=yes\n").is_none());
        // A `host-name` outside `[server]` is not ours to touch.
        assert!(remove_host_name("[publish]\nhost-name=x\n").is_none());
        assert!(remove_host_name("").is_none());
    }

    /// The two directions must be exact inverses: publish pins the name,
    /// unpublish takes it away again.
    #[test]
    fn publish_then_unpublish_leaves_no_override_behind() {
        let conf = "[server]\nuse-ipv4=yes\n\n[publish]\npublish-hinfo=no\n";
        let pinned = ensure_host_name(conf, "homevault").expect("pins the name");
        assert!(pinned.contains("host-name=homevault"));
        let stripped = remove_host_name(&pinned).expect("removes it again");
        assert!(!stripped.contains("host-name"), "{stripped}");
        assert!(stripped.contains("use-ipv4=yes"));
        assert!(stripped.contains("publish-hinfo=no"));
    }

    /// The flag the settings path writes and the reconciler reads.
    #[test]
    fn the_enabled_flag_round_trips() {
        let before = is_enabled();
        let moved = set_enabled(!before);
        assert!(moved, "flipping the flag must report a transition");
        assert_eq!(is_enabled(), !before);
        let moved_again = set_enabled(!before);
        assert!(!moved_again, "setting the same value is not a transition");
        assert_eq!(is_enabled(), !before);
        // Leave the process as we found it.
        set_enabled(before);
        assert_eq!(is_enabled(), before);
    }

    /// `status()` has to report the live flag, not a constant: the screen shows
    /// what the appliance actually does (ROADMAP 29.1).
    #[test]
    fn status_reports_the_enabled_flag() {
        let before = is_enabled();
        set_enabled(false);
        assert!(!status(8080).enabled, "a disabled appliance must say so");
        set_enabled(true);
        assert!(status(8080).enabled);
        set_enabled(before);
    }

    /// The path overrides exist so a test can point the whole publish/unpublish
    /// path at a temp directory. An exported-but-empty value must be ignored
    /// rather than resolving to `""`.
    #[test]
    fn the_avahi_paths_honour_their_override() {
        assert_eq!(avahi_conf_path(), AVAHI_CONF);
        assert_eq!(service_file_path(), SERVICE_FILE);

        std::env::set_var(AVAHI_CONF_ENV, "/tmp/hv-test/avahi.conf");
        std::env::set_var(SERVICE_FILE_ENV, "/tmp/hv-test/homevault.service");
        assert_eq!(avahi_conf_path(), "/tmp/hv-test/avahi.conf");
        assert_eq!(service_file_path(), "/tmp/hv-test/homevault.service");

        std::env::set_var(AVAHI_CONF_ENV, "   ");
        assert_eq!(avahi_conf_path(), AVAHI_CONF, "blank override is ignored");

        std::env::remove_var(AVAHI_CONF_ENV);
        std::env::remove_var(SERVICE_FILE_ENV);
    }

    /// ROADMAP X.2 debt in this module: every call used to be a bare
    /// `Command` invocation with no ceiling, so a `sudo` prompt waiting on a
    /// terminal that will never answer pinned the reconciler thread for ever.
    #[test]
    fn every_subprocess_is_bounded() {
        let src = include_str!("mdns.rs");
        for needle in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!(".", "status()"),
        ] {
            assert!(
                !src.contains(needle),
                "`{needle}` waits without a deadline — use `run_timed` (ROADMAP X.2)"
            );
        }
        assert!(
            src.contains("run_timed(") && src.contains("run_timed_with_stdin("),
            "the bounded helpers must be the ones in use"
        );
        // The ceilings must be ordered the way the module assumes: probes are
        // short, service actions are long.
        assert!(MDNS_PROBE_TIMEOUT < MDNS_SERVICE_TIMEOUT);
        assert!(MDNS_WRITE_TIMEOUT < MDNS_SERVICE_TIMEOUT);
    }

    #[test]
    fn test_status_shape_without_avahi() {
        // On a box with no avahi-daemon the status is still well-formed and
        // reports the canonical address (so the UI can show it regardless).
        let s = status(8080);
        let host = mdns_hostname();
        assert_eq!(s.fqdn, format!("{host}.local"));
        assert_eq!(s.http_url, format!("http://{host}.local:8080"));
        assert!(s.services.contains(&"_http._tcp".to_string()));
        if !s.available {
            assert!(!s.running);
        }
    }
}
