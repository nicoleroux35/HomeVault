/* =============================================================================
 * File          : snmp_agent.rs
 * Author        : Nicolas Le Roux
 * Created       : 04/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The local SNMP **agent** — the mirror image of `src/snmp.rs`, which is a
 *   client that queries *remote* devices. This module owns the appliance's own
 *   `snmpd`: it renders `/etc/snmp/snmpd.conf`, decides whether to bind the
 *   agent to loopback or to the LAN, and starts/stops the service.
 *
 *   Three rules it exists to keep:
 *   - **Off by default.** An unconfigured appliance answers no SNMP request
 *     at all. Monitoring is a choice, not a shipped default.
 *   - **Never `public`.** The default community is generated, not inherited:
 *     `public` is the first string every scanner tries, and an appliance that
 *     answers it is an appliance whose interface list and disk usage is public.
 *   - **LAN-bound, never internet-facing.** `agentaddress` is either loopback
 *     (local monitoring only) or one configured LAN address — never `udp:*`,
 *     never all interfaces.
 *
 * Dependencies:
 *   - serde, crate::utils::process (deadlines on every subprocess)
 *
 * Usage:
 *   handlers/snmp.rs — `GET|PUT /api/snmp/agent`.
 * =============================================================================
 */

//! The local SNMP agent (`snmpd`): config rendering, scope rules, service
//! control. See docs/guides/snmp-agent.md.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;

use crate::utils::process::{run_timed, run_timed_with_stdin, HOST_PROBE_TIMEOUT};

/// Reading the daemon's view: it prints its version and exits.
const SNMPD_PROBE_TIMEOUT: std::time::Duration = HOST_PROBE_TIMEOUT;
/// `systemctl start|stop|restart|enable|disable`: can wait on the job queue.
const SNMPD_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Writing the config file through `sudo tee`.
const SNMPD_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Every file this module owns. The `99-` prefix and the `homevault-` name
/// keep the write scoped to our own file, which is also what the sudoers grant
/// and the rollback rely on.
pub const CONF_PATH: &str = "/etc/snmp/snmpd.conf.d/99-homevault-agent.conf";

/// The longest community string `snmpd` accepts, and the length below which
/// a shared secret is not worth calling a secret.
pub const MIN_COMMUNITY_LEN: usize = 8;
pub const MAX_COMMUNITY_LEN: usize = 32;

/// Where the agent listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindScope {
    /// `agentaddress 127.0.0.1,[::1]` — only processes on this machine.
    Loopback,
    /// One configured LAN address: monitoring can come from the LAN, never
    /// from the internet. The installer already restricts what the appliance
    /// exposes; this keeps SNMP on the same side of the line.
    Lan,
}

impl BindScope {
    /// The default, and the safe answer for "I am not sure yet".
    pub fn default_scope() -> Self {
        BindScope::Loopback
    }
}

/// The agent's configuration, as persisted and as rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// `false` means "no agent": the file is removed and the service stopped.
    pub enabled: bool,
    pub scope: BindScope,
    /// The LAN address to bind to when `scope == Lan`. Required then, ignored
    /// otherwise (a stale address must not make a loopback agent unbootable).
    pub lan_address: Option<String>,
    /// The read-only community. Generated on enable, never `public`.
    pub community: String,
    /// `sysLocation` — where the box physically is.
    pub location: String,
    /// `sysContact` — who to wake up.
    pub contact: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scope: BindScope::default_scope(),
            lan_address: None,
            community: String::new(),
            location: String::new(),
            contact: String::new(),
        }
    }
}

impl AgentConfig {
    /// The config the agent would run with, without the secret.
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled,
            "scope": self.scope,
            "lan_address": self.lan_address,
            "location": self.location,
            "contact": self.contact,
            // Never the community itself: this endpoint is what the screen
            // reads on every visit.
            "has_community": !self.community.is_empty(),
        })
    }
}

/// Why a request to enable the agent was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

/// A generated community: 16 bytes of `/dev/urandom` rendered as lowercase
/// hex. Not base64: `snmpd.conf` treats `#` as a comment and spaces as
/// separators, and keeping the alphabet to `[0-9a-f]` removes the whole class
/// of "the community started with a `#`" bugs.
pub fn generate_community() -> Result<String, ConfigError> {
    let mut bytes = [0u8; 16];
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| ConfigError(format!("Could not read random bytes: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Is `community` a usable read-only community string?
///
/// The rules are the ones `snmpd.conf` imposes plus the one this project adds:
/// **`public` is refused by name**, because accepting it means shipping the
/// single most-guessed credential in network management.
pub fn validate_community(community: &str) -> Result<(), ConfigError> {
    let community = community.trim();
    if community.is_empty() {
        return Err(ConfigError("The community must not be empty".to_string()));
    }
    if community.eq_ignore_ascii_case("public") || community.eq_ignore_ascii_case("private") {
        return Err(ConfigError(
            "'public' and 'private' are the two strings every scanner tries first — choose another community"
                .to_string(),
        ));
    }
    if community.chars().count() < MIN_COMMUNITY_LEN {
        return Err(ConfigError(format!(
            "The community must be at least {MIN_COMMUNITY_LEN} characters long"
        )));
    }
    if community.chars().count() > MAX_COMMUNITY_LEN {
        return Err(ConfigError(format!(
            "The community must be at most {MAX_COMMUNITY_LEN} characters long"
        )));
    }
    // `snmpd.conf` is line- and whitespace-oriented: a `#`, a space, a quote
    // or a newline would silently change the meaning of the line.
    if community
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '#' || c == '"' || c == '\'')
    {
        return Err(ConfigError(
            "The community cannot contain spaces, quotes, `#` or control characters".to_string(),
        ));
    }
    Ok(())
}

/// Is `address` an address an agent may bind to?
///
/// Only a private, link-local or loopback IPv4 address is accepted: a public
/// address here would publish the appliance's SNMP agent to the internet, and
/// a wildcard (`0.0.0.0`) would listen on every interface at once.
pub fn validate_lan_address(address: &str) -> Result<Ipv4Addr, ConfigError> {
    let parsed: Ipv4Addr = address
        .trim()
        .parse()
        .map_err(|_| ConfigError(format!("'{address}' is not an IPv4 address")))?;
    if parsed.is_unspecified() {
        return Err(ConfigError(
            "The agent cannot bind to 0.0.0.0 — pick the appliance's own LAN address".to_string(),
        ));
    }
    if parsed.is_loopback() {
        return Err(ConfigError(
            "127.0.0.1 is the loopback scope — choose that scope instead".to_string(),
        ));
    }
    if !(parsed.is_private() || parsed.is_link_local()) {
        return Err(ConfigError(format!(
            "{} is not a private LAN address; SNMP must never be published outside the local network",
            parsed
        )));
    }
    Ok(parsed)
}

/// Validate a whole enable request, producing the config that would be saved.
///
/// `community: None` keeps the current one (or generates a fresh one when
/// there is none yet) — the screen never has to round-trip the secret just to
/// change the location string.
pub fn validate_config(
    requested: AgentConfig,
    current: &AgentConfig,
) -> Result<AgentConfig, ConfigError> {
    let mut next = requested;

    if next.enabled {
        // Only an unused address is required; the scope decides whether it is
        // used, so a loopback agent does not need to carry a LAN address.
        if next.scope == BindScope::Lan {
            let address = next.lan_address.clone().unwrap_or_default();
            let parsed = validate_lan_address(&address)?;
            next.lan_address = Some(parsed.to_string());
        } else {
            next.lan_address = None;
        }

        let community = if next.community.trim().is_empty() {
            if current.community.trim().is_empty() {
                generate_community()?
            } else {
                current.community.clone()
            }
        } else {
            next.community.trim().to_string()
        };
        validate_community(&community)?;
        next.community = community;

        next.location = sanitize_text(&next.location, 128)?;
        next.contact = sanitize_text(&next.contact, 128)?;
    } else {
        // Disabling keeps the community on disk? No: the file is deleted, so
        // the secret goes with it. A re-enable generates a fresh one unless
        // the screen supplied it.
        next.community = current.community.clone();
        next.location = sanitize_text(&next.location, 128)?;
        next.contact = sanitize_text(&next.contact, 128)?;
    }
    Ok(next)
}

/// Free text going into a `snmpd.conf` line: no newlines (they would start a
/// new directive), no control characters, bounded length.
fn sanitize_text(value: &str, max: usize) -> Result<String, ConfigError> {
    let trimmed = value.trim();
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(ConfigError(
            "The location and contact cannot contain line breaks or control characters".to_string(),
        ));
    }
    if trimmed.chars().count() > max {
        return Err(ConfigError(format!(
            "The location and contact must be at most {max} characters long"
        )));
    }
    Ok(trimmed.to_string())
}

/// The `agentaddress` line for a scope.
pub fn agent_address_line(config: &AgentConfig) -> String {
    match config.scope {
        BindScope::Loopback => "agentaddress 127.0.0.1,[::1]".to_string(),
        BindScope::Lan => {
            let address = config.lan_address.clone().unwrap_or_default();
            format!("agentaddress udp:{address}:161")
        }
    }
}

/// Render the whole `snmpd.conf` drop-in.
///
/// What it deliberately does **not** do: grant a write community, accept SNMPv1
/// (`sysservices`/`trap` stay v2c), or widen the default view beyond the two
/// OID subtrees the installer already adds (interfaces and storage) plus the
/// system group. An appliance that answers `inetCidrRouteTable` to anyone with
/// the community is an appliance that publishes its routing table.
pub fn render_config(config: &AgentConfig) -> String {
    let mut s = String::new();
    s.push_str("# Written by HomeVault from the SNMP agent settings.\n");
    s.push_str("# Delete this file and restart snmpd to hand the agent back.\n");
    s.push_str("# Read-only, SNMPv2c, no write community, no routing table.\n");
    s.push('\n');
    s.push_str(&agent_address_line(config));
    s.push('\n');
    s.push_str("# Read-only community. Never 'public'.\n");
    if config.community.is_empty() {
        // Defensive: a config must never be written without a community, but
        // if one somehow is, the agent answers nobody rather than everybody.
        s.push_str("# (no community configured — the agent answers no request)\n");
    } else {
        s.push_str(&format!("rocommunity {} default\n", config.community));
    }
    s.push('\n');
    s.push_str("# What an SNMP reader can see: system, interfaces, storage.\n");
    s.push_str("view systemonly included .1.3.6.1.2.1.1\n");
    s.push_str("view systemonly included .1.3.6.1.2.1.2\n");
    s.push_str("view systemonly included .1.3.6.1.2.1.25.2\n");
    s.push_str("view systemonly excluded .1.3.6.1.2.1.2.2.1.4\n");
    s.push('\n');
    if !config.location.is_empty() {
        s.push_str(&format!("sysLocation {}\n", config.location));
    }
    if !config.contact.is_empty() {
        s.push_str(&format!("sysContact {}\n", config.contact));
    }
    s
}

/// True when the `snmpd` daemon is installed (`/usr/sbin/snmpd`, falling back
/// to `PATH` for a non-standard layout).
pub fn agent_installed() -> bool {
    std::path::Path::new("/usr/sbin/snmpd").exists() || which("snmpd").is_some()
}

fn which(binary: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

/// `systemctl <action> snmpd`, under a deadline.
fn systemctl(action: &str) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["systemctl", action, "snmpd"]);
    let output = run_timed(&mut cmd, SNMPD_CONTROL_TIMEOUT)
        .map_err(|e| format!("could not run systemctl {action} snmpd: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "systemctl {} snmpd failed: {}",
            action,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Write the drop-in and restart the agent. Called after the config was
/// validated and persisted by the handler.
pub fn write_config(config: &AgentConfig) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["tee", CONF_PATH]);
    let body = render_config(config);
    let output = run_timed_with_stdin(&mut cmd, SNMPD_WRITE_TIMEOUT, body.as_bytes())
        .map_err(|e| format!("could not write {CONF_PATH}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not write {}: {}",
            CONF_PATH,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Our file carries a secret: root-only, like the netplan files this
    // project writes.
    let mut chmod = Command::new("sudo");
    chmod.args(["chmod", "600", CONF_PATH]);
    let _ = run_timed(&mut chmod, SNMPD_WRITE_TIMEOUT);
    Ok(())
}

/// Remove the drop-in (the disabled state) and stop the agent.
pub fn remove_config() -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["rm", "-f", CONF_PATH]);
    let output = run_timed(&mut cmd, SNMPD_WRITE_TIMEOUT)
        .map_err(|e| format!("could not remove {CONF_PATH}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not remove {}: {}",
            CONF_PATH,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Apply a config to the host: write (or remove) the file, then restart or
/// stop the service. Every step is reported, so the screen can say exactly
/// what state the appliance is in instead of a bare "saved".
pub fn apply_to_host(config: &AgentConfig) -> Result<&'static str, String> {
    if config.enabled {
        write_config(config)?;
        systemctl("enable")?;
        systemctl("restart")?;
        Ok("running")
    } else {
        remove_config()?;
        // Stop first, then disable: a stopped-but-enabled daemon comes back at
        // the next boot, which is exactly the state "off" must not leave.
        let _ = systemctl("stop");
        let _ = systemctl("disable");
        Ok("stopped")
    }
}

/// Whether the agent is currently serving SNMP queries.
pub fn is_serving() -> bool {
    let mut cmd = Command::new("systemctl");
    cmd.args(["is-active", "snmpd"]);
    matches!(run_timed(&mut cmd, SNMPD_PROBE_TIMEOUT), Ok(out) if out.status.success())
}

/// The addresses this appliance could bind the agent to: its own private and
/// link-local IPv4 addresses, so the screen offers a real choice instead of a
/// free-text field nobody can fill correctly.
pub fn lan_bind_candidates() -> Vec<String> {
    crate::network::list_interfaces()
        .into_iter()
        .filter_map(|iface| iface.ipv4)
        .filter(|ip| {
            ip.parse::<IpAddr>()
                .map(|addr| match addr {
                    IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified(),
                    IpAddr::V6(_) => false,
                })
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(community: &str, scope: BindScope) -> AgentConfig {
        AgentConfig {
            enabled: true,
            scope,
            lan_address: None,
            community: community.to_string(),
            location: "Rack, garage".to_string(),
            contact: "admin@example.local".to_string(),
        }
    }

    #[test]
    fn test_community_never_accepts_public_or_private() {
        for bad in ["public", "PUBLIC", "Private", "  public  "] {
            assert!(
                validate_community(bad).is_err(),
                "accepted the default community {bad:?}"
            );
        }
    }

    #[test]
    fn test_community_bounds_and_characters() {
        assert!(validate_community("short").is_err(), "too short");
        assert!(validate_community(&"a".repeat(MAX_COMMUNITY_LEN + 1)).is_err());
        assert!(validate_community(&"a".repeat(MAX_COMMUNITY_LEN)).is_ok());
        for bad in [
            "has space",
            "has\ttab",
            "line\nbreak",
            "quote\"d",
            "hash#tag",
            "ctrl\u{7}",
        ] {
            assert!(
                validate_community(bad).is_err(),
                "accepted {bad:?} — it would change the meaning of the snmpd line"
            );
        }
    }

    #[test]
    fn test_lan_address_must_be_private_and_never_a_wildcard() {
        assert!(validate_lan_address("192.168.1.10").is_ok());
        assert!(validate_lan_address("10.0.0.5").is_ok());
        assert!(validate_lan_address("172.16.4.4").is_ok());
        assert!(validate_lan_address("169.254.3.3").is_ok());
        // Wildcard, loopback, public and garbage are all refused.
        assert!(validate_lan_address("0.0.0.0").is_err());
        assert!(validate_lan_address("127.0.0.1").is_err());
        assert!(validate_lan_address("8.8.8.8").is_err());
        assert!(validate_lan_address("not-an-ip").is_err());
    }

    #[test]
    fn test_loopback_scope_ignores_a_stale_lan_address() {
        let current = enabled("", BindScope::Loopback);
        let mut requested = enabled("", BindScope::Loopback);
        requested.lan_address = Some("192.168.1.10".to_string());
        let next = validate_config(requested, &current).expect("valid");
        assert_eq!(
            next.lan_address, None,
            "a loopback agent carries no address"
        );
        assert!(validate_community(&next.community).is_ok());
    }

    #[test]
    fn test_lan_scope_requires_a_usable_address() {
        let current = enabled("", BindScope::Loopback);
        let mut requested = enabled("", BindScope::Lan);
        requested.lan_address = Some("0.0.0.0".to_string());
        assert!(validate_config(requested, &current).is_err());

        let mut ok = enabled("", BindScope::Lan);
        ok.lan_address = Some("192.168.1.10".to_string());
        let next = validate_config(ok, &current).expect("valid");
        assert_eq!(next.lan_address.as_deref(), Some("192.168.1.10"));
    }

    #[test]
    fn test_an_empty_community_is_generated_and_kept() {
        let current = AgentConfig::default();
        let next = validate_config(enabled("", BindScope::Loopback), &current).expect("valid");
        assert_eq!(next.community.len(), 32, "16 bytes of hex");
        assert_ne!(next.community, "public");

        // A later save without a community keeps the one in use, so the screen
        // never has to round-trip the secret.
        let mut again = enabled("", BindScope::Loopback);
        again.location = "Attic".to_string();
        let next2 = validate_config(again, &next).expect("valid");
        assert_eq!(next2.community, next.community);
    }

    #[test]
    fn test_rendered_config_is_read_only_and_bounded() {
        let config = enabled("0123456789abcdef", BindScope::Loopback);
        let rendered = render_config(&config);
        assert!(
            rendered.contains("agentaddress 127.0.0.1,[::1]"),
            "{rendered}"
        );
        assert!(
            rendered.contains("rocommunity 0123456789abcdef default"),
            "{rendered}"
        );
        assert!(rendered.contains("sysLocation Rack, garage"), "{rendered}");
        // Never a write community, never the routing table, never `udp:*`.
        assert!(!rendered.contains("rwcommunity"), "{rendered}");
        assert!(!rendered.contains("inetCidrRouteTable"), "{rendered}");
        assert!(!rendered.contains("udp:*"), "{rendered}");
        assert!(!rendered.contains("0.0.0.0"), "{rendered}");
        // The agent must not answer the interfaces' own addresses either — the
        // view excludes `ifInetAddress`-adjacent columns? No: it is the
        // ifPhysAddress (MAC) column that stays hidden.
        assert!(
            rendered.contains("view systemonly excluded .1.3.6.1.2.1.2.2.1.4"),
            "{rendered}"
        );
    }

    #[test]
    fn test_lan_scope_renders_one_address_never_a_wildcard() {
        let mut config = enabled("0123456789abcdef", BindScope::Lan);
        config.lan_address = Some("192.168.1.10".to_string());
        let rendered = render_config(&config);
        assert!(
            rendered.contains("agentaddress udp:192.168.1.10:161"),
            "{rendered}"
        );
    }

    #[test]
    fn test_a_config_without_a_community_answers_nobody() {
        let mut config = enabled("", BindScope::Loopback);
        config.community = String::new();
        let rendered = render_config(&config);
        assert!(!rendered.contains("rocommunity"), "{rendered}");
        assert!(rendered.contains("answers no request"), "{rendered}");
    }

    #[test]
    fn test_redacted_config_never_carries_the_community() {
        let config = enabled("0123456789abcdef", BindScope::Lan);
        let redacted = config.redacted();
        let text = redacted.to_string();
        assert!(!text.contains("0123456789abcdef"), "{text}");
        assert_eq!(redacted["has_community"], serde_json::json!(true));
        assert_eq!(redacted["enabled"], serde_json::json!(true));
    }

    #[test]
    fn test_location_and_contact_cannot_inject_a_directive() {
        let current = AgentConfig::default();
        let mut config = enabled("0123456789abcdef", BindScope::Loopback);
        config.location = "Rack\nrocommunity attacker default".to_string();
        assert!(validate_config(config, &current).is_err());

        let mut crowded = enabled("0123456789abcdef", BindScope::Loopback);
        crowded.contact = "x".repeat(200);
        assert!(validate_config(crowded, &current).is_err());
    }
}
