/* =============================================================================
 * File          : upnp.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPnP-IGD port forwarding through the `upnpc` client that ships with
 *   miniupnpc (phase C3). This module owns the safe half: validate a mapping,
 *   render the exact argv, and parse the router's answer. It never opens a
 *   port on its own — the handler asks, and every mutation must be confirmed.
 *
 *   Four rules it exists to keep:
 *   - **Read before write.** Listing the router's mappings is the default
 *     action and changes nothing.
 *   - **A mapping is a LAN target.** The internal client must be a private,
 *     link-local or ULA address; loopback, public and unspecified addresses are
 *     refused, because they either cannot work or would open the wrong door.
 *   - **No shell, no surprises.** Arguments are validated and passed one by
 *     one; a description cannot carry a quote, a newline or a control
 *     character into `upnpc`'s argv.
 *   - **Reversible.** Every mapping HomeVault adds is labelled with its own
 *     prefix and can be removed from the same list; a lease is required
 *     (permanent is allowed but never the default).
 *
 * Dependencies:
 *   - crate::utils::process (bounded `upnpc` runs),
 *     crate::dependencies (capability probe), crate::network (address policy)
 *
 * Usage:
 *   handlers/upnp.rs — `GET /api/network/upnp`, `POST|DELETE
 *   /api/network/upnp/mappings`. See docs/guides/upnp.md.
 * ============================================================================= */

//! UPnP-IGD port forwarding: validation, argv rendering, listing parser.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::process::Command;
use std::time::Duration;

use crate::utils::process::run_timed;

/// The package that provides `upnpc`. Absent, the card names it instead of
/// showing a form that cannot work.
pub const REQUIRED_PACKAGE: &str = "miniupnpc";

/// `upnpc` talks to the router over HTTP; a healthy gateway answers in under a
/// second, and a silent one must not hold the screen.
const UPNPC_TIMEOUT: Duration = Duration::from_secs(8);

/// A router table with more entries than this is not a home LAN.
pub const MAX_MAPPINGS: usize = 100;

/// Description prefix so a user can recognise what HomeVault added and remove
/// exactly that, rather than guessing among their own entries.
pub const DESCRIPTION_PREFIX: &str = "HomeVault";

/// The longest description `upnpc` and the router UI render comfortably.
const MAX_DESCRIPTION_LEN: usize = 64;

/// Lease bounds in seconds: a minute to a week. `0` means permanent.
const MIN_LEASE: u32 = 60;
const MAX_LEASE: u32 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    /// `upnpc` spells the protocol in upper case on both `-a` and `-d`.
    pub fn as_upnpc(self) -> &'static str {
        match self {
            Protocol::Tcp => "TCP",
            Protocol::Udp => "UDP",
        }
    }
}

/// One mapping as the router reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMapping {
    pub protocol: Protocol,
    pub external_port: u16,
    pub internal_host: String,
    pub internal_port: u16,
    pub description: String,
    /// `0` = permanent.
    pub lease_seconds: u32,
}

/// The router's state, as far as this feature needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpnpSnapshot {
    pub gateway: Option<String>,
    pub external_ip: Option<String>,
    pub local_address: Option<String>,
    pub mappings: Vec<PortMapping>,
}

/// What the screen asks to add.
#[derive(Debug, Clone, Deserialize)]
pub struct NewMapping {
    pub protocol: Protocol,
    pub external_port: u16,
    pub internal_host: String,
    pub internal_port: u16,
    #[serde(default)]
    pub description: String,
    /// `0` = permanent; absent means one hour.
    #[serde(default = "default_lease")]
    pub lease_seconds: u32,
    /// Opening a port on the router is a deliberate act.
    #[serde(default)]
    pub confirmed: bool,
}

fn default_lease() -> u32 {
    3600
}

/// What the screen asks to remove.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoveMapping {
    pub protocol: Protocol,
    pub external_port: u16,
    #[serde(default)]
    pub confirmed: bool,
}

/// A mapping that passed validation, with its rendered description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMapping {
    pub mapping: PortMapping,
}

impl ValidatedMapping {
    /// The exact `upnpc` argv for `-a`.
    pub fn add_args(&self) -> Vec<String> {
        let mapping = &self.mapping;
        vec![
            "-a".to_string(),
            mapping.internal_host.clone(),
            mapping.internal_port.to_string(),
            mapping.external_port.to_string(),
            mapping.protocol.as_upnpc().to_string(),
            mapping.description.clone(),
            mapping.lease_seconds.to_string(),
        ]
    }
}

/// Validate an add request before anything reaches the router.
pub fn validate_new(input: &NewMapping) -> Result<ValidatedMapping, String> {
    if !input.confirmed {
        return Err(
            "Opening a port on the router must be confirmed; nothing was changed.".to_string(),
        );
    }
    if input.external_port == 0 {
        return Err("The external port must be between 1 and 65535.".to_string());
    }
    if input.internal_port == 0 {
        return Err("The internal port must be between 1 and 65535.".to_string());
    }

    let internal: IpAddr = input
        .internal_host
        .trim()
        .parse()
        .map_err(|_| "The internal address must be an IPv4 or IPv6 address.".to_string())?;
    if internal.is_loopback() {
        return Err(
            "The internal address cannot be loopback — the router cannot reach it.".to_string(),
        );
    }
    if !crate::network::is_private_address(internal) {
        return Err(
            "The internal address must be a private, link-local or unique-local address on your \
             network."
                .to_string(),
        );
    }

    if input.lease_seconds != 0 && !(MIN_LEASE..=MAX_LEASE).contains(&input.lease_seconds) {
        return Err(format!(
            "The lease must be permanent (0) or between {MIN_LEASE} and {MAX_LEASE} seconds."
        ));
    }

    let description = render_description(&input.description)?;

    Ok(ValidatedMapping {
        mapping: PortMapping {
            protocol: input.protocol,
            external_port: input.external_port,
            internal_host: internal.to_string(),
            internal_port: input.internal_port,
            description,
            lease_seconds: input.lease_seconds,
        },
    })
}

/// Validate a removal request. It names exactly one `(protocol, port)` pair,
/// which is what `upnpc -d` takes; no wildcard is possible.
pub fn validate_remove(input: &RemoveMapping) -> Result<(), String> {
    if !input.confirmed {
        return Err("Removing a port mapping must be confirmed; nothing was changed.".to_string());
    }
    if input.external_port == 0 {
        return Err("The external port must be between 1 and 65535.".to_string());
    }
    Ok(())
}

/// The `upnpc -d` argv for one mapping.
pub fn remove_args(protocol: Protocol, external_port: u16) -> Vec<String> {
    vec![
        "-d".to_string(),
        external_port.to_string(),
        protocol.as_upnpc().to_string(),
    ]
}

/// A description that is safe in argv and recognisable in the router's table.
fn render_description(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let text = if raw.is_empty() {
        DESCRIPTION_PREFIX.to_string()
    } else if raw.starts_with(DESCRIPTION_PREFIX) {
        raw.to_string()
    } else {
        format!("{DESCRIPTION_PREFIX}: {raw}")
    };
    if text.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(format!(
            "The description is limited to {MAX_DESCRIPTION_LEN} characters, including the \
             '{DESCRIPTION_PREFIX}' prefix."
        ));
    }
    if text
        .chars()
        .any(|c| !c.is_ascii() || c.is_control() || matches!(c, '\'' | '"' | '\\'))
    {
        return Err(
            "The description must be printable ASCII without quotes or backslashes.".to_string(),
        );
    }
    Ok(text)
}

/// Run `upnpc` with a bound and return stdout, or a sentence naming the
/// failure. Arguments are already validated and passed one by one.
fn run(args: &[String]) -> Result<String, String> {
    let mut command = Command::new("upnpc");
    command.args(args);
    let output =
        run_timed(&mut command, UPNPC_TIMEOUT).map_err(|e| format!("Could not run upnpc: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = if stderr.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        stderr.trim().to_string()
    };
    Err(if detail.is_empty() {
        "The router refused the request.".to_string()
    } else {
        detail
    })
}

/// Is the `upnpc` client installed?
pub fn available() -> bool {
    crate::dependencies::binary_available("upnpc", "/usr/bin/upnpc")
}

/// Read the router's current state. Read-only: this is `upnpc -l`.
pub fn snapshot() -> Result<UpnpSnapshot, String> {
    let output = run(&["-l".to_string()])?;
    parse_listing(&output)
}

/// Add one mapping. The caller re-reads the router afterwards, so a succeeded
/// write is never reported as a failure just because the follow-up listing
/// failed.
pub fn add(input: &ValidatedMapping) -> Result<(), String> {
    run(&input.add_args()).map(|_| ())
}

/// Remove one mapping.
pub fn remove(protocol: Protocol, external_port: u16) -> Result<(), String> {
    run(&remove_args(protocol, external_port)).map(|_| ())
}

/// Parse `upnpc -l`. The format is `%2d %s %5hu->%s:%-5hu '%s' '%s'` in
/// miniupnpc, but the client has changed spacing between versions, so tokens
/// are read tolerantly and a line that is not a mapping is ignored rather than
/// failing the whole list.
pub fn parse_listing(output: &str) -> Result<UpnpSnapshot, String> {
    let mut snapshot = UpnpSnapshot::default();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(value) = trimmed
            .strip_prefix("ExternalIPAddress = ")
            .or_else(|| trimmed.strip_prefix("ExternalIPAddress: "))
        {
            snapshot.external_ip = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed
            .strip_prefix("Local LAN ip address : ")
            .or_else(|| trimmed.strip_prefix("Local LAN ip address: "))
        {
            snapshot.local_address = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Found valid IGD : ") {
            snapshot.gateway = Some(value.trim().to_string());
            continue;
        }

        if let Some(mapping) = parse_mapping_line(trimmed) {
            if snapshot.mappings.len() < MAX_MAPPINGS {
                snapshot.mappings.push(mapping);
            }
        }
    }

    if snapshot.gateway.is_none() && snapshot.external_ip.is_none() && snapshot.mappings.is_empty()
    {
        return Err(
            "No UPnP gateway answered. Check that UPnP is enabled on the router and that this \
             network allows multicast discovery."
                .to_string(),
        );
    }
    Ok(snapshot)
}

/// One `upnpc -l` mapping row.
fn parse_mapping_line(line: &str) -> Option<PortMapping> {
    // Drop the leading row index when present.
    let rest = line
        .split_once(char::is_whitespace)
        .filter(|(index, _)| index.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, rest)| rest.trim())
        .unwrap_or(line);

    let (protocol, rest) = split_protocol(rest)?;
    let (before_arrow, after_arrow) = rest.split_once("->")?;
    let before_arrow = before_arrow.trim();
    let after_arrow = after_arrow.trim();

    let external_port = leading_digits(before_arrow)?;

    let authority = after_arrow.split_whitespace().next()?;
    let (internal_host, internal_port) = split_authority(authority)?;

    let mut quoted = quoted_values(after_arrow);
    let description = quoted
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(DESCRIPTION_PREFIX)
        .to_string();
    let lease_seconds = quoted
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);

    Some(PortMapping {
        protocol,
        external_port,
        internal_host,
        internal_port,
        description,
        lease_seconds,
    })
}

/// `TCP`/`UDP`, accepting the lower-case form some builds print.
fn split_protocol(rest: &str) -> Option<(Protocol, &str)> {
    if let Some(rest) = rest
        .strip_prefix("TCP")
        .or_else(|| rest.strip_prefix("tcp"))
    {
        return Some((Protocol::Tcp, rest));
    }
    let rest = rest
        .strip_prefix("UDP")
        .or_else(|| rest.strip_prefix("udp"))?;
    Some((Protocol::Udp, rest))
}

fn leading_digits(raw: &str) -> Option<u16> {
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|port| *port > 0)
}

/// `host:port`, accepting a bracketed IPv6 internal client.
fn split_authority(raw: &str) -> Option<(String, u16)> {
    if let Some(rest) = raw.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?.parse::<u16>().ok()?;
        return Some((format!("[{host}]"), port));
    }
    let (host, port) = raw.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    Some((host.to_string(), port))
}

/// Every `'…'` value in a row, in order (description, then lease).
fn quoted_values(raw: &str) -> impl Iterator<Item = &str> {
    let mut rest = raw;
    std::iter::from_fn(move || {
        let start = rest.find('\'')?;
        let after = &rest[start + 1..];
        let end = after.find('\'')?;
        let value = &after[..end];
        rest = &after[end + 1..];
        Some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_mapping() -> NewMapping {
        NewMapping {
            protocol: Protocol::Tcp,
            external_port: 8080,
            internal_host: "192.168.1.50".to_string(),
            internal_port: 80,
            description: "web".to_string(),
            lease_seconds: 3600,
            confirmed: true,
        }
    }

    const LISTING: &str = "\
List of UPNP devices found on the network :
 desc: http://192.168.1.1:5000/rootDesc.xml
 st: urn:schemas-upnp-org:device:InternetGatewayDevice:1

Found valid IGD : http://192.168.1.1:5000/ctl/IPConn
Local LAN ip address : 192.168.1.10
ExternalIPAddress = 203.0.113.5
 0 TCP   8080->192.168.1.50:80     'HomeVault: web' '3600'
 1 UDP  5353->192.168.1.10:5353   'Jellyfin' '0'
";

    #[test]
    fn test_parse_listing_reads_the_gateway_and_every_mapping() {
        let snapshot = parse_listing(LISTING).unwrap();
        assert_eq!(
            snapshot.gateway.as_deref(),
            Some("http://192.168.1.1:5000/ctl/IPConn")
        );
        assert_eq!(snapshot.local_address.as_deref(), Some("192.168.1.10"));
        assert_eq!(snapshot.external_ip.as_deref(), Some("203.0.113.5"));
        assert_eq!(snapshot.mappings.len(), 2);
        assert_eq!(snapshot.mappings[0].protocol, Protocol::Tcp);
        assert_eq!(snapshot.mappings[0].external_port, 8080);
        assert_eq!(snapshot.mappings[0].internal_host, "192.168.1.50");
        assert_eq!(snapshot.mappings[0].internal_port, 80);
        assert_eq!(snapshot.mappings[0].description, "HomeVault: web");
        assert_eq!(snapshot.mappings[0].lease_seconds, 3600);
        assert_eq!(snapshot.mappings[1].protocol, Protocol::Udp);
        assert_eq!(snapshot.mappings[1].lease_seconds, 0);
    }

    #[test]
    fn test_parse_listing_tolerates_an_empty_table() {
        let snapshot = parse_listing(
            "Found valid IGD : http://192.168.1.1:5000/ctl/IPConn\nExternalIPAddress = 203.0.113.5\n",
        )
        .unwrap();
        assert!(snapshot.mappings.is_empty());
    }

    #[test]
    fn test_parse_listing_refuses_a_router_that_did_not_answer() {
        let err = parse_listing("No IGD found\n").unwrap_err();
        assert!(err.contains("No UPnP gateway"), "{err}");
    }

    #[test]
    fn test_parse_listing_ignores_a_malformed_row_but_keeps_the_rest() {
        let snapshot = parse_listing(
            "Found valid IGD : http://gw/ctl\n 0 TCP not-a-row\n 1 TCP 9000->192.168.1.9:9000 'x' '60'\n",
        )
        .unwrap();
        assert_eq!(snapshot.mappings.len(), 1);
        assert_eq!(snapshot.mappings[0].external_port, 9000);
    }

    #[test]
    fn test_protocol_is_lowercase_on_the_wire() {
        assert_eq!(serde_json::to_string(&Protocol::Tcp).unwrap(), "\"tcp\"");
        assert_eq!(
            serde_json::from_str::<Protocol>("\"udp\"").unwrap(),
            Protocol::Udp
        );
        assert_eq!(Protocol::Tcp.as_upnpc(), "TCP");
    }

    #[test]
    fn test_add_args_are_exactly_the_documented_shape() {
        let validated = validate_new(&new_mapping()).unwrap();
        assert_eq!(
            validated.add_args(),
            vec![
                "-a",
                "192.168.1.50",
                "80",
                "8080",
                "TCP",
                "HomeVault: web",
                "3600"
            ]
        );
    }

    #[test]
    fn test_remove_args_name_one_pair() {
        assert_eq!(remove_args(Protocol::Udp, 5353), vec!["-d", "5353", "UDP"]);
    }

    #[test]
    fn test_confirmation_is_required_for_both_mutations() {
        let mut add = new_mapping();
        add.confirmed = false;
        assert!(validate_new(&add).unwrap_err().contains("confirmed"));
        assert!(validate_remove(&RemoveMapping {
            protocol: Protocol::Tcp,
            external_port: 8080,
            confirmed: false,
        })
        .unwrap_err()
        .contains("confirmed"));
    }

    #[test]
    fn test_port_zero_is_refused() {
        let mut add = new_mapping();
        add.external_port = 0;
        assert!(validate_new(&add).is_err());
        add = new_mapping();
        add.internal_port = 0;
        assert!(validate_new(&add).is_err());
        assert!(validate_remove(&RemoveMapping {
            protocol: Protocol::Tcp,
            external_port: 0,
            confirmed: true,
        })
        .is_err());
    }

    #[test]
    fn test_internal_client_must_be_a_lan_address() {
        for host in ["8.8.8.8", "0.0.0.0", "127.0.0.1", "::1", "example.com"] {
            let mut add = new_mapping();
            add.internal_host = host.to_string();
            assert!(validate_new(&add).is_err(), "{host}");
        }
    }

    #[test]
    fn test_private_and_link_local_clients_are_accepted() {
        for host in [
            "192.168.1.50",
            "10.0.0.5",
            "172.16.4.4",
            "fd00::10",
            "fe80::1",
        ] {
            let mut add = new_mapping();
            add.internal_host = host.to_string();
            assert!(validate_new(&add).is_ok(), "{host}");
        }
    }

    #[test]
    fn test_description_is_prefixed_and_bounded() {
        let mut add = new_mapping();
        add.description = String::new();
        assert_eq!(validate_new(&add).unwrap().mapping.description, "HomeVault");
        add.description = "  NAS  ".to_string();
        assert_eq!(
            validate_new(&add).unwrap().mapping.description,
            "HomeVault: NAS"
        );
        add.description = "a".repeat(MAX_DESCRIPTION_LEN);
        assert!(validate_new(&add).is_err());
    }

    #[test]
    fn test_description_refuses_argv_and_table_breakouts() {
        for bad in ["a'b", "a\"b", "a\\b", "a\nb", "café"] {
            let mut add = new_mapping();
            add.description = bad.to_string();
            assert!(validate_new(&add).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_lease_bounds() {
        for lease in [0, 60, 3600, MAX_LEASE] {
            let mut add = new_mapping();
            add.lease_seconds = lease;
            assert!(validate_new(&add).is_ok(), "{lease}");
        }
        for lease in [1, 59, MAX_LEASE + 1] {
            let mut add = new_mapping();
            add.lease_seconds = lease;
            assert!(validate_new(&add).is_err(), "{lease}");
        }
    }

    #[test]
    fn test_an_ipv6_client_is_accepted_and_bracketed_round_trips() {
        let mut add = new_mapping();
        add.internal_host = "fd00::10".to_string();
        let validated = validate_new(&add).unwrap();
        assert_eq!(validated.mapping.internal_host, "fd00::10");
        assert_eq!(validated.add_args()[1], "fd00::10");
    }

    #[test]
    fn test_default_lease_is_one_hour() {
        let input: NewMapping = serde_json::from_value(serde_json::json!({
            "protocol": "tcp",
            "external_port": 8080,
            "internal_host": "192.168.1.50",
            "internal_port": 80,
            "confirmed": true
        }))
        .unwrap();
        assert_eq!(input.lease_seconds, 3600);
    }
}
