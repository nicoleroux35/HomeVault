/* =============================================================================
 * File          : local_dns.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The appliance's own DNS server, served by the `dnsmasq` instance the DHCP
 *   card already drives — one daemon, one drop-in, two configuration halves.
 *   This module owns the DNS half: the local A/CNAME records, the upstream
 *   forwarders, the `address=/domain/address` entries and the switch that
 *   tells DHCP clients to resolve through this appliance.
 *
 *   Four rules it exists to keep:
 *   - **Off by default.** An appliance that answers DNS for a LAN can break the
 *     LAN's resolution if it is wrong, so it is a choice, not a shipped default.
 *   - **A name never escapes the box by accident.** `domain-needed` and
 *     `bogus-priv` keep single-label and reverse lookups of private ranges from
 *     being forwarded, so the LAN's device names are not leaked to the ISP's
 *     resolver. `no-resolv` means the forwarders listed here are the only ones
 *     used, never a surprise `/etc/resolv.conf`.
 *   - **LAN-bound.** `bind-interfaces` on one named interface plus
 *     `local-service`, so the appliance never answers a resolver that reached it
 *     from the internet.
 *   - **What dnsmasq cannot do is refused, not written.** A `cname=` whose
 *     target dnsmasq does not know is silently ignored by the daemon; here it is
 *     a refused save with the reason.
 *
 * Dependencies:
 *   - serde, crate::config::paths, crate::network (interface candidates)
 *
 * Usage:
 *   handlers/local_dns.rs — `GET|PUT /api/network/dns-server`. The rendered
 *   section is appended to the shared drop-in by
 *   `dhcp_server::render_conf`. See docs/guides/local-dns.md.
 * ============================================================================= */

//! The appliance's own DNS server (`dnsmasq`): records, forwarders, overrides
//! and the rendered configuration section. See docs/guides/local-dns.md.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::PathBuf;

use crate::config::paths;

/// The package that backs this feature. Absent, the card names it instead of
/// showing a form that cannot work.
pub const REQUIRED_PACKAGE: &str = "dnsmasq";

/// How many upstream forwarders a home LAN needs, and a bound that keeps the
/// rendered file readable.
pub const MAX_FORWARDERS: usize = 3;

/// Bounds on the two lists. A DNS server for a house never needs more, and an
/// unbounded list is an unbounded `dnsmasq` reload.
pub const MAX_RECORDS: usize = 100;
pub const MAX_OVERRIDES: usize = 100;

/// DNS name limits (RFC 1035).
const MAX_NAME_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// `IFNAMSIZ - 1`: the longest interface name the kernel accepts.
const MAX_INTERFACE_LEN: usize = 15;

/// What kind of answer a local record produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordKind {
    /// `host-record=<name>,<address>` — one name, one address.
    A,
    /// `cname=<alias>,<target>` — an alias of a name this appliance serves.
    Cname,
}

/// One local name record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    /// The name to answer for (`nas.lan`).
    pub name: String,
    pub kind: RecordKind,
    /// An IPv4 address for [`RecordKind::A`], the alias target for
    /// [`RecordKind::Cname`].
    pub value: String,
}

/// `address=/<domain>/<address>`: every name in (and under) the domain is
/// answered here and never forwarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainOverride {
    /// The domain, optionally prefixed with `.` to match subdomains only.
    pub domain: String,
    pub address: String,
}

/// The DNS half of the appliance's `dnsmasq` configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalDnsConfig {
    /// `false` means "no local DNS": the daemon runs only for DHCP, or not at
    /// all.
    pub enabled: bool,
    /// The one interface that answers queries. Empty means "not chosen yet".
    pub interface: String,
    /// Tell DHCP clients to resolve through this appliance. dnsmasq already
    /// sends its own address for option 6 once the explicit list is gone, so
    /// this is a switch on the *DHCP* section, and it only means anything while
    /// the DHCP server is on.
    pub serve_dhcp_clients: bool,
    /// Upstream resolvers, used for everything this appliance does not answer
    /// itself.
    pub forwarders: Vec<String>,
    /// Local names.
    pub records: Vec<DnsRecord>,
    /// Domains answered here instead of being forwarded.
    pub overrides: Vec<DomainOverride>,
    /// Explicit confirmation, required to turn the server on.
    pub confirmed: bool,
}

// Written out rather than derived on purpose: this `false`/empty/`false` list is
// the security posture of the feature, and it must stay visible in review.
#[allow(clippy::derivable_impls)]
impl Default for LocalDnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interface: String::new(),
            serve_dhcp_clients: false,
            forwarders: Vec::new(),
            records: Vec::new(),
            overrides: Vec::new(),
            confirmed: false,
        }
    }
}

impl LocalDnsConfig {
    /// What the screen reads. Nothing secret lives in here (a resolver address
    /// is not a credential), so this is the whole config.
    pub fn payload(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// Why a configuration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Normalise and validate a DNS name.
///
/// Lowercased, one trailing dot dropped (FQDN notation is accepted, stored
/// canonically), then RFC 1035 label rules. Anything that would change the
/// meaning of a `dnsmasq.conf` line — a space, a `/`, a `#`, a newline, a
/// wildcard — is refused by the charset rather than escaped, because dnsmasq
/// has no quoting that would make them safe here.
pub fn validate_hostname(raw: &str) -> Result<String, ConfigError> {
    let trimmed = raw.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return Err(ConfigError("A name cannot be empty".to_string()));
    }
    if trimmed.len() > MAX_NAME_LEN {
        return Err(ConfigError(format!(
            "The name '{}' is longer than {MAX_NAME_LEN} characters",
            trimmed
        )));
    }
    if trimmed.contains('*') {
        return Err(ConfigError(
            "Wildcards are not accepted — list the names you want answered".to_string(),
        ));
    }
    let name = trimmed.to_ascii_lowercase();
    for label in name.split('.') {
        if label.is_empty() {
            return Err(ConfigError(format!(
                "'{name}' has an empty label (a doubled or leading dot)"
            )));
        }
        if label.len() > MAX_LABEL_LEN {
            return Err(ConfigError(format!(
                "'{name}' has a label longer than {MAX_LABEL_LEN} characters"
            )));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(ConfigError(format!(
                "'{name}' has a label that starts or ends with '-'"
            )));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(ConfigError(format!(
                "'{name}' may only contain letters, digits, '-' and '.'"
            )));
        }
    }
    Ok(name)
}

/// Normalise and validate a domain for an `address=` entry.
///
/// A leading dot is meaningful to dnsmasq — `/example.com/` matches the name
/// *and* everything under it, `/.example.com/` matches only the subdomains — so
/// it is preserved rather than stripped, and a domain that is nothing but `""`
/// (which would hijack every query on the appliance) is refused.
pub fn validate_domain(raw: &str) -> Result<String, ConfigError> {
    let trimmed = raw.trim();
    let (prefix, rest) = match trimmed.strip_prefix('.') {
        Some(rest) => (".", rest),
        None => ("", trimmed),
    };
    let name = validate_hostname(rest)?;
    Ok(format!("{prefix}{name}"))
}

/// An interface name, as far as `dnsmasq.conf` and the kernel are concerned.
fn validate_interface_name(raw: &str) -> Result<(), ConfigError> {
    if raw.len() > MAX_INTERFACE_LEN {
        return Err(ConfigError(format!(
            "'{raw}' is longer than the {MAX_INTERFACE_LEN} characters a Linux interface name can have"
        )));
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
    {
        return Err(ConfigError(format!(
            "'{raw}' is not an interface name (letters, digits, '-', '_', '.' and ':' only)"
        )));
    }
    Ok(())
}

/// An address this appliance may hand out, in a record or in an override.
fn validate_answer_address(raw: &str) -> Result<String, ConfigError> {
    let trimmed = raw.trim();
    let parsed: Ipv4Addr = trimmed
        .parse()
        .map_err(|_| ConfigError(format!("'{trimmed}' is not an IPv4 address")))?;
    if parsed.is_unspecified() {
        return Err(ConfigError(
            "0.0.0.0 is not an address to answer with — give the device's real address".to_string(),
        ));
    }
    if parsed.is_multicast() || parsed.is_broadcast() {
        return Err(ConfigError(format!(
            "{parsed} cannot be the address of a host"
        )));
    }
    Ok(parsed.to_string())
}

/// An upstream resolver.
///
/// A loopback address is refused by name: on this appliance it is either the
/// server itself (an endless loop) or a stub resolver belonging to the very
/// daemon being configured, and either way the LAN loses resolution with no
/// hint as to why.
fn validate_forwarder(raw: &str) -> Result<String, ConfigError> {
    let trimmed = raw.trim();
    let parsed: Ipv4Addr = trimmed
        .parse()
        .map_err(|_| ConfigError(format!("'{trimmed}' is not an IPv4 address")))?;
    if parsed.is_loopback() {
        return Err(ConfigError(
            "127.0.0.1 as an upstream would send every query back to this appliance — give the router's address (or another resolver on the LAN)".to_string(),
        ));
    }
    validate_answer_address(&parsed.to_string())
}

/// Validate a whole configuration and return the normalised one to persist.
///
/// Everything is validated even while the server is off, so a half-typed list
/// cannot be stored and then fail at the moment it is switched on.
pub fn validate_config(requested: &LocalDnsConfig) -> Result<LocalDnsConfig, ConfigError> {
    let mut next = requested.clone();

    // --- Interface -------------------------------------------------------
    next.interface = next.interface.trim().to_string();
    if !next.interface.is_empty() {
        validate_interface_name(&next.interface)?;
    }

    // --- Forwarders ------------------------------------------------------
    if next.forwarders.len() > MAX_FORWARDERS {
        return Err(ConfigError(format!(
            "At most {MAX_FORWARDERS} upstream forwarders — a home LAN does not need more, and every one of them lengthens every lookup"
        )));
    }
    let mut forwarders: Vec<String> = Vec::with_capacity(next.forwarders.len());
    for raw in &next.forwarders {
        let address = validate_forwarder(raw)?;
        if forwarders.contains(&address) {
            return Err(ConfigError(format!(
                "{address} is listed twice as an upstream forwarder"
            )));
        }
        forwarders.push(address);
    }
    next.forwarders = forwarders;

    // --- Local records ---------------------------------------------------
    if next.records.len() > MAX_RECORDS {
        return Err(ConfigError(format!("At most {MAX_RECORDS} local names")));
    }
    let mut normalised: Vec<(String, RecordKind, String)> = Vec::with_capacity(next.records.len());
    for record in &next.records {
        let name = validate_hostname(&record.name)?;
        if normalised.iter().any(|(existing, _, _)| *existing == name) {
            return Err(ConfigError(format!(
                "'{name}' is listed twice — a name can only have one answer"
            )));
        }
        let value = match record.kind {
            RecordKind::A => validate_answer_address(&record.value)?,
            RecordKind::Cname => validate_hostname(&record.value)?,
        };
        normalised.push((name, record.kind, value));
    }
    // dnsmasq ignores a `cname=` whose target it does not know, silently. Every
    // alias therefore has to point at an address record from this same list.
    for (name, kind, value) in &normalised {
        if *kind != RecordKind::Cname {
            continue;
        }
        if value == name {
            return Err(ConfigError(format!(
                "'{name}' cannot be an alias of itself"
            )));
        }
        if !normalised
            .iter()
            .any(|(other, other_kind, _)| other == value && *other_kind == RecordKind::A)
        {
            return Err(ConfigError(format!(
                "The alias '{name}' points at '{value}', which this appliance does not serve — dnsmasq ignores an alias whose target it cannot resolve. Add an address record for '{value}' first"
            )));
        }
    }
    next.records = normalised
        .into_iter()
        .map(|(name, kind, value)| DnsRecord { name, kind, value })
        .collect();

    // --- Domain overrides ------------------------------------------------
    if next.overrides.len() > MAX_OVERRIDES {
        return Err(ConfigError(format!(
            "At most {MAX_OVERRIDES} answered domains"
        )));
    }
    let mut overrides: Vec<DomainOverride> = Vec::with_capacity(next.overrides.len());
    for entry in &next.overrides {
        let domain = validate_domain(&entry.domain)?;
        let address = validate_answer_address(&entry.address)?;
        if overrides.iter().any(|existing| existing.domain == domain) {
            return Err(ConfigError(format!(
                "'{domain}' is listed twice as an answered domain"
            )));
        }
        // A domain that is also a single-name record is two answers for one
        // question; dnsmasq would quietly prefer the domain entry.
        let bare = domain.trim_start_matches('.');
        if next
            .records
            .iter()
            .any(|record| record.name == bare || record.name.ends_with(&format!(".{bare}")))
        {
            return Err(ConfigError(format!(
                "'{bare}' already has a local name record — a domain entry and a name record for the same name would answer differently"
            )));
        }
        overrides.push(DomainOverride { domain, address });
    }
    next.overrides = overrides;

    // --- The gates -------------------------------------------------------
    if next.enabled {
        if next.interface.is_empty() {
            return Err(ConfigError(
                "Choose the interface that answers DNS queries".to_string(),
            ));
        }
        if next.forwarders.is_empty() {
            return Err(ConfigError(
                "Add at least one upstream forwarder — without one this appliance would answer only for the names listed here and nothing else, and the rest of the LAN's lookups would fail".to_string(),
            ));
        }
        if !next.confirmed {
            return Err(ConfigError(
                "explicit confirmation required to serve DNS for the LAN".to_string(),
            ));
        }
    } else if next.serve_dhcp_clients {
        return Err(ConfigError(
            "Answering for the LAN's DHCP clients only means anything while the DNS server is on"
                .to_string(),
        ));
    }

    Ok(next)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// The DNS half of the shared `dnsmasq` drop-in.
///
/// The listening policy (`interface=`, `bind-interfaces`) is deliberately *not*
/// here: it is shared with the DHCP half and owned by
/// `dhcp_server::render_conf`, so the two features can never disagree about
/// which interfaces the single daemon answers on.
pub fn render_section(config: &LocalDnsConfig) -> String {
    let mut s = String::new();
    s.push_str("# Local DNS server (Settings > Security > Local DNS server).\n");
    if !config.enabled {
        s.push_str("# Off: no name is answered and no query is forwarded.\n");
        return s;
    }

    // Never /etc/resolv.conf: the forwarders below are the whole story, so what
    // this appliance resolves cannot change under it when the host's own
    // resolver is reconfigured.
    s.push_str("no-resolv\n");
    // Do not forward a bare name (there is no dot to resolve) …
    s.push_str("domain-needed\n");
    // … and do not send reverse lookups of private ranges upstream: that is
    // how a home LAN's device names leak to the ISP's resolver.
    s.push_str("bogus-priv\n");
    // Answer only resolvers on a directly attached subnet, never one that
    // reached the appliance from the internet.
    s.push_str("local-service\n");

    for forwarder in &config.forwarders {
        s.push_str(&format!("server={forwarder}\n"));
    }

    for record in &config.records {
        match record.kind {
            RecordKind::A => s.push_str(&format!("host-record={},{}\n", record.name, record.value)),
            RecordKind::Cname => s.push_str(&format!("cname={},{}\n", record.name, record.value)),
        }
    }

    for entry in &config.overrides {
        s.push_str(&format!("address=/{}/{}\n", entry.domain, entry.address));
    }

    s
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Where the DNS half is stored. Its own document, next to the DHCP one, so
/// turning one off never rewrites the other.
pub fn config_path() -> PathBuf {
    paths::base_dir().join("data/local_dns.json")
}

/// The saved configuration, or the safe default (off) when there is none.
pub async fn load_persisted() -> Result<LocalDnsConfig, String> {
    let path = config_path();
    if !path.exists() {
        return Ok(LocalDnsConfig::default());
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content).map_err(|e| format!("failed to parse {}: {}", path.display(), e))
}

/// Persist the configuration (the caller has already validated it).
pub async fn save_persisted(config: &LocalDnsConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed to serialize the local DNS settings: {}", e))?;
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("failed to write {}: {}", path.display(), e))
}

// ---------------------------------------------------------------------------
// Host state
// ---------------------------------------------------------------------------

/// The interfaces a DNS server could plausibly answer on: every interface with
/// an IPv4 address, loopback excluded.
///
/// Returned so the screen offers a real choice instead of a free-text field;
/// an interface with no address yet (a freshly created bond) is left out
/// because `dnsmasq` cannot bind it.
pub fn interface_candidates() -> Vec<String> {
    crate::network::list_interfaces()
        .into_iter()
        .filter(|iface| {
            iface
                .ipv4
                .as_deref()
                .map(|ip| {
                    ip.parse::<Ipv4Addr>()
                        .map(|addr| !addr.is_loopback() && !addr.is_unspecified())
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|iface| iface.name)
        .collect()
}

/// Parse `nameserver` lines out of a `resolv.conf` body, keeping what could be
/// a usable forwarder (IPv4, not loopback, no duplicates).
pub fn parse_resolv_conf(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("nameserver") else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(parsed) = value.parse::<Ipv4Addr>() else {
            continue;
        };
        if parsed.is_loopback() || parsed.is_unspecified() {
            continue;
        }
        let rendered = parsed.to_string();
        if !out.contains(&rendered) {
            out.push(rendered);
        }
    }
    out
}

/// The host's current upstream resolvers, offered as suggestions so the admin
/// does not have to know the router's address by heart.
pub fn system_forwarders() -> Vec<String> {
    match std::fs::read_to_string("/etc/resolv.conf") {
        Ok(text) => {
            let mut found = parse_resolv_conf(&text);
            found.truncate(MAX_FORWARDERS);
            found
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, kind: RecordKind, value: &str) -> DnsRecord {
        DnsRecord {
            name: name.to_string(),
            kind,
            value: value.to_string(),
        }
    }

    fn enabled() -> LocalDnsConfig {
        LocalDnsConfig {
            enabled: true,
            interface: "eth0".to_string(),
            serve_dhcp_clients: false,
            forwarders: vec!["192.168.1.1".to_string()],
            records: vec![],
            overrides: vec![],
            confirmed: true,
        }
    }

    // --- hostnames --------------------------------------------------------

    #[test]
    fn test_validate_hostname_normalises_and_accepts() {
        assert_eq!(validate_hostname("NAS.Lan").unwrap(), "nas.lan");
        assert_eq!(validate_hostname("  nas.lan.  ").unwrap(), "nas.lan");
        assert_eq!(validate_hostname("nas").unwrap(), "nas");
        assert_eq!(validate_hostname("a-b.c-d.lan").unwrap(), "a-b.c-d.lan");
        assert_eq!(
            validate_hostname(&format!("{}.lan", "a".repeat(MAX_LABEL_LEN))).unwrap(),
            format!("{}.lan", "a".repeat(MAX_LABEL_LEN))
        );
    }

    #[test]
    fn test_validate_hostname_refuses_what_would_change_a_conf_line() {
        for bad in [
            "",
            "   ",
            "nas lan",
            "nas\tlan",
            "nas\nlan",
            "nas.lan#comment",
            "nas/lan",
            "*.lan",
            "nas..lan",
            ".lan",
            "nas-.lan",
            "-nas.lan",
            "nas_lan",
            "nas.lan/x",
        ] {
            assert!(
                validate_hostname(bad).is_err(),
                "accepted {bad:?} — it would change the meaning of the dnsmasq line"
            );
        }
    }

    #[test]
    fn test_validate_hostname_refuses_oversized_names() {
        assert!(validate_hostname(&"a".repeat(MAX_LABEL_LEN + 1)).is_err());
        let long = format!("{}.{}", "a".repeat(60), "b".repeat(60).repeat(5));
        assert!(validate_hostname(&long).is_err());
    }

    #[test]
    fn test_validate_domain_keeps_the_subdomain_only_dot() {
        assert_eq!(validate_domain("lan").unwrap(), "lan");
        assert_eq!(validate_domain(".lan").unwrap(), ".lan");
        assert_eq!(validate_domain("Corp.Lan.").unwrap(), "corp.lan");
        // Digits and hyphens are fine; a wildcard or an empty domain (which
        // would hijack every query) is not.
        assert_eq!(validate_domain("192-168.lan").unwrap(), "192-168.lan");
        assert!(validate_domain("").is_err());
        assert!(validate_domain(".").is_err());
        assert!(validate_domain("*").is_err());
    }

    // --- addresses --------------------------------------------------------

    #[test]
    fn test_answer_address_refuses_what_is_not_a_host() {
        assert_eq!(
            validate_answer_address("192.168.1.5").unwrap(),
            "192.168.1.5"
        );
        assert_eq!(validate_answer_address(" 10.0.0.2 ").unwrap(), "10.0.0.2");
        for bad in [
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "not-an-ip",
            "1.2.3",
        ] {
            assert!(
                validate_answer_address(bad).is_err(),
                "accepted {bad:?} as a host address"
            );
        }
    }

    #[test]
    fn test_a_loopback_forwarder_is_refused_by_name() {
        let err = validate_forwarder("127.0.0.1").unwrap_err();
        assert!(err.0.contains("back to this appliance"), "{err}");
        assert!(validate_forwarder("192.168.1.1").is_ok());
        assert!(validate_forwarder("9.9.9.9").is_ok());
        assert!(validate_forwarder("0.0.0.0").is_err());
    }

    // --- the whole configuration -----------------------------------------

    #[test]
    fn test_an_enabled_server_needs_an_interface_forwarders_and_confirmation() {
        let mut config = enabled();
        config.interface = String::new();
        assert!(validate_config(&config).is_err(), "no interface");

        let mut config = enabled();
        config.forwarders.clear();
        let err = validate_config(&config).unwrap_err();
        assert!(err.0.contains("upstream forwarder"), "{err}");

        let mut config = enabled();
        config.confirmed = false;
        let err = validate_config(&config).unwrap_err();
        assert!(err.0.contains("confirmation"), "{err}");
    }

    #[test]
    fn test_off_requires_no_gate_but_still_validates_the_lists() {
        let config = LocalDnsConfig {
            records: vec![record("nas.lan", RecordKind::A, "192.168.1.5")],
            ..LocalDnsConfig::default()
        };
        assert!(validate_config(&config).is_ok());

        // A typo is caught while the server is off, not when it is switched on.
        let typo = LocalDnsConfig {
            records: vec![record("nas lan", RecordKind::A, "192.168.1.5")],
            ..LocalDnsConfig::default()
        };
        assert!(validate_config(&typo).is_err());
    }

    #[test]
    fn test_advertising_to_dhcp_clients_requires_the_server_to_be_on() {
        let config = LocalDnsConfig {
            serve_dhcp_clients: true,
            ..LocalDnsConfig::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.0.contains("while the DNS server is on"), "{err}");

        let mut on = enabled();
        on.serve_dhcp_clients = true;
        assert!(validate_config(&on).is_ok());
    }

    #[test]
    fn test_a_name_has_exactly_one_answer() {
        let mut config = enabled();
        config.records = vec![
            record("nas.lan", RecordKind::A, "192.168.1.5"),
            record("NAS.lan", RecordKind::A, "192.168.1.6"),
        ];
        let err = validate_config(&config).unwrap_err();
        assert!(err.0.contains("listed twice"), "{err}");
    }

    #[test]
    fn test_an_alias_must_point_at_a_name_this_appliance_serves() {
        let mut config = enabled();
        config.records = vec![record("media.lan", RecordKind::Cname, "nas.lan")];
        let err = validate_config(&config).unwrap_err();
        assert!(err.0.contains("does not serve"), "{err}");

        config.records = vec![
            record("nas.lan", RecordKind::A, "192.168.1.5"),
            record("media.lan", RecordKind::Cname, "nas.lan"),
        ];
        assert!(validate_config(&config).is_ok());

        // Nor may it point at itself.
        let mut loopy = enabled();
        loopy.records = vec![record("media.lan", RecordKind::Cname, "media.lan")];
        assert!(validate_config(&loopy).is_err());
    }

    #[test]
    fn test_a_domain_entry_and_a_name_record_cannot_answer_the_same_name() {
        let mut config = enabled();
        config.records = vec![record("nas.lan", RecordKind::A, "192.168.1.5")];
        config.overrides = vec![DomainOverride {
            domain: "nas.lan".to_string(),
            address: "192.168.1.9".to_string(),
        }];
        let err = validate_config(&config).unwrap_err();
        assert!(err.0.contains("already has a local name record"), "{err}");
    }

    #[test]
    fn test_duplicate_domains_and_forwarders_are_refused() {
        let mut config = enabled();
        config.forwarders = vec!["192.168.1.1".to_string(), "192.168.1.1".to_string()];
        assert!(validate_config(&config).is_err());

        let mut config = enabled();
        config.overrides = vec![
            DomainOverride {
                domain: "lan".to_string(),
                address: "192.168.1.1".to_string(),
            },
            DomainOverride {
                domain: "LAN".to_string(),
                address: "192.168.1.2".to_string(),
            },
        ];
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_the_interface_name_is_bounded_and_cannot_inject() {
        let mut config = enabled();
        config.interface = "eth0\nserver=6.6.6.6".to_string();
        assert!(validate_config(&config).is_err());

        let mut config = enabled();
        config.interface = "a".repeat(MAX_INTERFACE_LEN + 1);
        assert!(validate_config(&config).is_err());

        let mut config = enabled();
        config.interface = "enp3s0.42".to_string();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_lists_are_bounded() {
        let mut config = enabled();
        config.forwarders = vec![
            "192.168.1.1".to_string(),
            "192.168.1.2".to_string(),
            "192.168.1.3".to_string(),
            "192.168.1.4".to_string(),
        ];
        assert!(validate_config(&config).is_err());

        let mut config = enabled();
        config.records = (0..=MAX_RECORDS)
            .map(|i| record(&format!("n{i}.lan"), RecordKind::A, "192.168.1.5"))
            .collect();
        assert!(validate_config(&config).is_err());
    }

    // --- rendering --------------------------------------------------------

    #[test]
    fn test_render_off_says_so_and_emits_no_directive() {
        let rendered = render_section(&LocalDnsConfig::default());
        assert!(rendered.contains("Off:"), "{rendered}");
        for directive in ["server=", "no-resolv", "host-record=", "address=/"] {
            assert!(
                !rendered.contains(directive),
                "{directive} leaked into the off section: {rendered}"
            );
        }
    }

    #[test]
    fn test_render_carries_the_privacy_and_binding_directives() {
        let rendered = render_section(&enabled());
        for directive in ["no-resolv", "domain-needed", "bogus-priv", "local-service"] {
            assert!(
                rendered.contains(directive),
                "{directive} missing: {rendered}"
            );
        }
        assert!(rendered.contains("server=192.168.1.1"), "{rendered}");
        // The listening policy belongs to the DHCP half of the drop-in.
        assert!(!rendered.contains("interface="), "{rendered}");
        assert!(!rendered.contains("bind-interfaces"), "{rendered}");
    }

    #[test]
    fn test_render_emits_one_line_per_record_and_override() {
        let mut config = enabled();
        config.records = vec![
            record("nas.lan", RecordKind::A, "192.168.1.5"),
            record("media.lan", RecordKind::Cname, "nas.lan"),
        ];
        config.overrides = vec![DomainOverride {
            domain: ".corp.lan".to_string(),
            address: "10.0.0.1".to_string(),
        }];
        let rendered = render_section(&config);
        assert!(
            rendered.contains("host-record=nas.lan,192.168.1.5"),
            "{rendered}"
        );
        assert!(rendered.contains("cname=media.lan,nas.lan"), "{rendered}");
        assert!(
            rendered.contains("address=/.corp.lan/10.0.0.1"),
            "{rendered}"
        );
    }

    // --- resolv.conf parsing ---------------------------------------------

    #[test]
    fn test_parse_resolv_conf_keeps_usable_ipv4_nameservers_only() {
        let text = "\
# comment
nameserver 192.168.1.1
nameserver 127.0.0.53
nameserver 9.9.9.9
nameserver fe80::1
search lan
nameserver 192.168.1.1
nameserver
";
        assert_eq!(
            parse_resolv_conf(text),
            vec!["192.168.1.1".to_string(), "9.9.9.9".to_string()]
        );
    }

    #[test]
    fn test_parse_resolv_conf_of_an_empty_file_is_empty() {
        assert!(parse_resolv_conf("").is_empty());
        assert!(parse_resolv_conf("nameserver 127.0.0.1\n").is_empty());
    }

    // --- persistence ------------------------------------------------------

    // The load/save round-trip lives in `tests/local_dns_integration.rs`: it
    // needs its own `DATA_DIRECTORY`, and mutating the process environment from
    // a unit test would leak into every other test in the binary.
    #[test]
    fn test_the_document_lives_next_to_the_dhcp_one() {
        let path = config_path();
        assert!(
            path.ends_with("data/local_dns.json"),
            "unexpected config path: {}",
            path.display()
        );
    }

    #[test]
    fn test_the_default_is_off() {
        let default = LocalDnsConfig::default();
        assert!(!default.enabled, "local DNS must ship off");
        assert!(!default.serve_dhcp_clients);
        assert!(default.forwarders.is_empty());
    }
}
