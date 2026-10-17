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

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

use crate::shares::{ShareType, SharesService};

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
/// The DNS-SD service definition HomeVault owns. Avahi loads every
/// `*.service` in this directory; this is the only one we write.
pub const SERVICE_FILE: &str = "/etc/avahi/services/homevault.service";

/// How often the background reconciler re-derives the advertised set (which
/// shares are enabled can change from the Shares page at any time) and
/// rewrites [`SERVICE_FILE`] if — and only if — the content changed.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Root-write helpers (mirrors network.rs::write_as_root)
// ---------------------------------------------------------------------------

fn sudo_tee(path: &str, content: &str) -> Result<(), String> {
    let mut child = Command::new("sudo")
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("sudo_tee: no stdin")?
        .write_all(content.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
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
    let out = Command::new("sudo")
        .args(["rm", "-f", path])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn systemctl(action: &str) -> Result<(), String> {
    let out = Command::new("sudo")
        .args(["systemctl", action, "avahi-daemon"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Whether a systemd unit is currently active. Read-only, needs no root
/// (same query `dependencies.rs` uses).
fn unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `avahi-daemon` binary present on this host (matches `dependencies.rs`).
pub fn avahi_available() -> bool {
    Command::new("which")
        .arg("avahi-daemon")
        .output()
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
    let conf = std::fs::read_to_string(AVAHI_CONF).unwrap_or_default();
    if let Some(new_conf) = ensure_host_name(&conf, &mdns_hostname()) {
        sudo_tee(AVAHI_CONF, &new_conf)?;
        needs_restart = true;
    }

    // 2. Describe the services.
    let desired = render_service_file(web_port, &detect_services());
    let current = std::fs::read_to_string(SERVICE_FILE).unwrap_or_default();
    if current != desired {
        sudo_tee(SERVICE_FILE, &desired)?;
        needs_reload = true;
    }

    // 3. Apply.
    if needs_restart {
        systemctl("restart")?;
    } else if needs_reload {
        // `avahi-daemon --reload` picks up service-file changes without a
        // full restart; fall back to systemctl reload if the binary call
        // isn't permitted.
        let direct = Command::new("sudo")
            .args(["avahi-daemon", "--reload"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !direct {
            systemctl("reload")?;
        }
    }
    Ok(())
}

/// Remove HomeVault's mDNS advertisement (service file) and reload Avahi.
/// Leaves the `host-name` override in place — harmless on its own and cheap
/// to reuse if publishing is turned back on.
pub fn unpublish() -> Result<(), String> {
    if !avahi_available() || !std::path::Path::new(SERVICE_FILE).exists() {
        return Ok(());
    }
    sudo_rm(SERVICE_FILE)?;
    let _ = systemctl("reload");
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
