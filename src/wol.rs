/* =============================================================================
 * File          : wol.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Wake-on-LAN: build and send the 102-byte magic packet that wakes a
 *   powered-off NIC, choose the interface whose subnet it belongs to, and
 *   parse the "wake first" pre-step a scheduled task may carry. Everything
 *   that decides *what* leaves the box is pure and unit-tested; only the UDP
 *   send itself touches the network.
 *
 * Dependencies:
 *   - tokio (net), serde, crate::network (interface inventory)
 *
 * Usage:
 *   handlers/wol.rs (Settings card) and scheduled_tasks_scheduler.rs
 *   ("Wake first"); see docs/guides/power.md.
 * =============================================================================
 */

//! Wake-on-LAN (see docs/guides/power.md).
//!
//! HomeVault is often installed on a machine that sleeps, and "wake the box,
//! then run the 03:00 backup" is the natural pairing. Three pieces:
//!
//! * **The packet** — [`magic_packet`] is the standard 6 × `0xFF` prefix
//!   followed by the target MAC sixteen times (102 bytes). Pure, so its exact
//!   bytes are asserted in a test.
//! * **The destination** — a WOL frame is a *link-layer* broadcast, so it has
//!   to leave the interface that owns the target's subnet; [`destination_for`]
//!   resolves the interface's **directed broadcast** from its address and
//!   netmask (or the admin's explicit override, for a relay on another VLAN).
//! * **An honest answer** — [`wake`] reports `sent, not confirmed`. A UDP
//!   datagram has no acknowledgement and the target is powered off by
//!   definition, so claiming "woken" would be a lie in the one case the feature
//!   exists for.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::{Deserialize, Serialize};

use crate::network::NetworkInterface;

/// Length of a magic packet: six `0xFF` bytes plus the MAC sixteen times.
pub const MAGIC_PACKET_LEN: usize = 6 + 16 * 6;

/// Ports a magic packet is sent to. 9 (`discard`) is the convention and 7
/// (`echo`) is the older one; some NICs/relays only listen on one of them, and
/// two small datagrams cost nothing.
pub const WOL_PORTS: [u16; 2] = [9, 7];

/// Longest a "wake first" attachment may wait before running its task.
///
/// A cold boot from WOL is typically 20–90 seconds; ten minutes is the point
/// past which the task should have been scheduled later instead of pinning a
/// run open.
pub const MAX_WAKE_DELAY_SECONDS: u64 = 600;

/// Saved hosts are a short list picked in a dropdown; a longer one is a sign
/// nobody is curating it.
pub const MAX_HOSTS: usize = 64;

/// A host the box may wake (`wol_hosts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct WolHost {
    pub id: i64,
    pub label: String,
    /// Canonical form: uppercase, colon-separated (`AA:BB:CC:DD:EE:FF`).
    pub mac: String,
    /// Optional destination override (an IPv4/IPv6 literal) used instead of the
    /// selected interface's directed broadcast.
    pub broadcast: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// --------------------------------------------------------------------------
// The packet
// --------------------------------------------------------------------------

/// Parse a MAC address in any of the forms an admin is likely to paste.
///
/// Accepts `AA:BB:CC:DD:EE:FF`, `aa-bb-cc-dd-ee-ff`, Cisco `aabb.ccdd.eeff`
/// and a bare 12-hex-digit string, and returns the six bytes.
///
/// Refuses:
/// * a length that is not six octets;
/// * a non-hex character (a typo must not silently become a different NIC);
/// * a **multicast/broadcast** address (the low bit of the first octet set) —
///   the all-ones address is the broadcast of every network, and waking "every
///   machine" is never what the admin meant;
/// * the all-zero address (no NIC has it).
pub fn parse_mac(value: &str) -> Result<[u8; 6], String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("the MAC address is required".to_string());
    }
    let digits: String = trimmed
        .chars()
        .filter(|c| *c != ':' && *c != '-' && *c != '.' && !c.is_whitespace())
        .collect();
    if digits.len() != 12 {
        return Err(format!(
            "'{trimmed}' is not a MAC address (expected 12 hex digits, e.g. AA:BB:CC:DD:EE:FF)"
        ));
    }
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("'{trimmed}' contains a character that is not hex"));
    }
    let mut bytes = [0u8; 6];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16)
            .map_err(|_| format!("'{trimmed}' is not a MAC address"))?;
    }
    if bytes == [0u8; 6] {
        return Err("00:00:00:00:00:00 is not a network card".to_string());
    }
    if bytes[0] & 0x01 != 0 {
        return Err(format!(
            "'{trimmed}' is a multicast/broadcast address, not a network card address"
        ));
    }
    Ok(bytes)
}

/// The canonical text form of a MAC: uppercase, colon-separated.
pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The 102-byte magic packet for one NIC.
pub fn magic_packet(mac: &[u8; 6]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(MAGIC_PACKET_LEN);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(mac);
    }
    debug_assert_eq!(packet.len(), MAGIC_PACKET_LEN);
    packet
}

// --------------------------------------------------------------------------
// Subnets and interfaces
// --------------------------------------------------------------------------

/// One IPv4 interface reduced to what a broadcast needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IfaceV4 {
    pub name: String,
    pub address: Ipv4Addr,
    pub prefix: u8,
    /// The **directed** broadcast of this interface (`192.168.1.255` for a
    /// `192.168.1.0/24`), which is what a WOL frame must be addressed to: it
    /// leaves *this* interface and reaches every NIC on that segment.
    pub broadcast: Ipv4Addr,
    pub up: bool,
}

/// Netmask → prefix length, refusing a non-contiguous mask.
///
/// `255.255.255.0` is `/24`; `255.0.255.0` is a typo in a config file and has
/// no prefix, so it is refused instead of silently rounded.
pub fn netmask_to_prefix(mask: Ipv4Addr) -> Option<u8> {
    let bits = u32::from(mask);
    let prefix = bits.leading_ones() as u8;
    // All bits after the prefix must be zero for the mask to be contiguous.
    if prefix < 32 && bits << prefix != 0 {
        return None;
    }
    Some(prefix)
}

/// Prefix length → netmask (`None` above 32).
pub fn prefix_to_netmask(prefix: u8) -> Option<Ipv4Addr> {
    if prefix > 32 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some(Ipv4Addr::from(mask))
}

/// The directed broadcast address of `address`/`prefix`.
///
/// `/0` has no usable directed broadcast (it would be `255.255.255.255`), so it
/// is reported as the limited broadcast; `/32` has none at all (`None`).
pub fn directed_broadcast(address: Ipv4Addr, prefix: u8) -> Option<Ipv4Addr> {
    if prefix > 32 {
        return None;
    }
    if prefix == 32 {
        return None;
    }
    let host_mask = if prefix == 0 {
        u32::MAX
    } else {
        !(u32::MAX << (32 - prefix))
    };
    Some(Ipv4Addr::from(u32::from(address) | host_mask))
}

/// Whether two addresses are in the same subnet.
pub fn same_subnet(a: Ipv4Addr, b: Ipv4Addr, prefix: u8) -> bool {
    match prefix_to_netmask(prefix) {
        Some(mask) => u32::from(a) & u32::from(mask) == u32::from(b) & u32::from(mask),
        None => false,
    }
}

/// The IPv4 interfaces that can carry a broadcast, from the inventory the
/// Network screen already reads (`crate::network::list_interfaces`).
///
/// Loopback and interfaces without an address are dropped: a WOL frame sent on
/// `lo` reaches nothing.
pub fn ipv4_interfaces(interfaces: &[NetworkInterface]) -> Vec<IfaceV4> {
    interfaces
        .iter()
        .filter_map(|iface| {
            // A loopback (or other non-physical) interface never carries a WOL
            // frame: its "subnet" is not reachable, so skip it before parsing.
            if iface.iface_type == crate::network::InterfaceType::Loopback {
                return None;
            }
            let address = iface.ipv4.as_deref()?.parse::<Ipv4Addr>().ok()?;
            let mask = iface
                .ipv4_netmask
                .as_deref()
                .and_then(|m| m.parse::<Ipv4Addr>().ok())
                .or_else(|| {
                    iface.ipv6_prefix.and_then(|_| None)
                    // No netmask reported: assume a /24, the home default,
                    // rather than refusing to wake anything.
                })
                .unwrap_or(Ipv4Addr::new(255, 255, 255, 0));
            let prefix = netmask_to_prefix(mask)?;
            let broadcast = directed_broadcast(address, prefix).unwrap_or(Ipv4Addr::BROADCAST);
            Some(IfaceV4 {
                name: iface.name.clone(),
                address,
                prefix,
                broadcast,
                up: iface.status == crate::network::InterfaceStatus::Up,
            })
        })
        .collect()
}

/// Pick the interface a WOL frame should leave through.
///
/// * with a `target`, the **up** interface whose subnet contains it (the
///   machine being woken is on that segment);
/// * without one, the first **up** interface that is not loopback — a home box
///   has one LAN, and its directed broadcast is the honest default;
/// * an interface that exists but is down is never chosen.
pub fn select_interface<'a>(
    ifaces: &'a [IfaceV4],
    target: Option<Ipv4Addr>,
) -> Option<&'a IfaceV4> {
    match target {
        Some(target) => ifaces
            .iter()
            .find(|iface| iface.up && same_subnet(iface.address, target, iface.prefix)),
        None => ifaces.iter().find(|iface| iface.up),
    }
}

/// Where a host's magic packet goes, and which interface (if any) was chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Destination {
    pub address: IpAddr,
    /// The interface whose directed broadcast was used; `None` means the admin
    /// configured an explicit destination.
    pub interface: Option<String>,
}

/// Resolve the destination for one host.
///
/// Order of preference:
/// 1. the host's explicit `broadcast` (a relay, another VLAN) — validated as an
///    IP literal by [`validate_broadcast`], so no name lookup can hang a wake;
/// 2. the directed broadcast of the selected interface;
/// 3. the limited broadcast `255.255.255.255`, which works on a single-segment
///    LAN even when the inventory is incomplete.
pub fn destination_for(host: &WolHost, ifaces: &[IfaceV4]) -> Result<Destination, String> {
    if let Some(override_value) = host
        .broadcast
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let address = override_value
            .parse::<IpAddr>()
            .map_err(|_| format!("'{override_value}' is not an IP address"))?;
        return Ok(Destination {
            address,
            interface: None,
        });
    }
    match select_interface(ifaces, None) {
        Some(iface) => Ok(Destination {
            address: IpAddr::V4(iface.broadcast),
            interface: Some(iface.name.clone()),
        }),
        None => Ok(Destination {
            address: IpAddr::V4(Ipv4Addr::BROADCAST),
            interface: None,
        }),
    }
}

// --------------------------------------------------------------------------
// Validation
// --------------------------------------------------------------------------

/// A host label: short, printable, and not something that breaks a dropdown.
pub fn validate_label(label: &str) -> Result<String, String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err("a name is required for the host".to_string());
    }
    if trimmed.chars().count() > 64 {
        return Err("the name must be 64 characters or fewer".to_string());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("the name contains control characters".to_string());
    }
    Ok(trimmed.to_string())
}

/// An explicit destination: an IP literal only.
///
/// A host name would be resolved at wake time — a DNS lookup between "click"
/// and "packet", which can hang, and which can answer with an address on the
/// wrong segment. A broadcast address is what the field is *for*, so it is
/// accepted; a host address is accepted too (a relay or a test sink).
pub fn validate_broadcast(value: &str) -> Result<Option<String>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let address = trimmed
        .parse::<IpAddr>()
        .map_err(|_| format!("'{trimmed}' is not an IP address"))?;
    Ok(Some(address.to_string()))
}

// --------------------------------------------------------------------------
// Sending
// --------------------------------------------------------------------------

/// Outcome of a wake attempt, worded for the API and the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WakeReport {
    pub mac: String,
    pub destination: String,
    pub interface: Option<String>,
    pub ports: Vec<u16>,
}

impl WakeReport {
    /// The sentence shown to the admin. Never "woken": a UDP datagram is
    /// unacknowledged and the target is off by definition.
    pub fn message(&self) -> String {
        match &self.interface {
            Some(iface) => format!(
                "Magic packet sent to {} on {} ({}). This only confirms it left: switch the machine on \
                 and check that it answers — a NIC with Wake-on-LAN disabled ignores it.",
                self.destination, iface, self.mac
            ),
            None => format!(
                "Magic packet sent to {} ({}). This only confirms it left: switch the machine on and \
                 check that it answers — a NIC with Wake-on-LAN disabled ignores it.",
                self.destination, self.mac
            ),
        }
    }
}

/// Send one magic packet to `destination` on every [`WOL_PORTS`] entry.
///
/// One socket, `SO_BROADCAST` enabled (the kernel refuses to send to a
/// broadcast address without it), one datagram per port. There is no deadline
/// on `send_to` because UDP does not block on a peer; the call can only fail
/// immediately (unreachable route, permission).
pub async fn send_magic(mac: &[u8; 6], destination: SocketAddr) -> Result<(), String> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("could not open a local socket: {e}"))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("could not enable broadcast: {e}"))?;
    let packet = magic_packet(mac);
    socket
        .send_to(&packet, destination)
        .await
        .map_err(|e| format!("the magic packet could not be sent to {destination}: {e}"))?;
    Ok(())
}

/// Wake one saved host: resolve the destination, send on both ports, and
/// report what left.
pub async fn wake(host: &WolHost, ifaces: &[IfaceV4]) -> Result<WakeReport, String> {
    let mac = parse_mac(&host.mac)?;
    let destination = destination_for(host, ifaces)?;
    for port in WOL_PORTS {
        send_magic(&mac, SocketAddr::new(destination.address, port)).await?;
    }
    Ok(WakeReport {
        mac: format_mac(&mac),
        destination: destination.address.to_string(),
        interface: destination.interface,
        ports: WOL_PORTS.to_vec(),
    })
}

/// The live interface inventory, reduced to the IPv4 view a broadcast needs.
///
/// `list_interfaces` reads `/sys/class/net` (no subprocess), so this is cheap
/// enough to call from a request.
pub fn live_interfaces() -> Vec<IfaceV4> {
    ipv4_interfaces(&crate::network::list_interfaces())
}

// --------------------------------------------------------------------------
// "Wake first" on scheduled tasks
// --------------------------------------------------------------------------

/// The pre-step a scheduled task may carry: wake a saved host, then wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakePlan {
    pub host_id: i64,
    #[serde(default)]
    pub delay_seconds: u64,
}

/// Parse the optional `"wake"` block out of a scheduled-task payload.
///
/// Shape: `{"type": "command", …, "wake": {"host_id": 2, "delay_seconds": 90}}`
/// — **not** part of `TaskAction`, because it is a pre-step every task type may
/// carry rather than a fifth task type. An unknown field inside the block is
/// refused (`deny_unknown_fields`): a typo like `delay` instead of
/// `delay_seconds` must not silently become "wake and run immediately".
pub fn parse_wake_plan(payload_json: &str) -> Result<Option<WakePlan>, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Block {
        host_id: i64,
        #[serde(default)]
        delay_seconds: u64,
    }

    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        wake: Option<Block>,
    }

    let envelope: Envelope =
        serde_json::from_str(payload_json).map_err(|e| format!("Invalid task payload: {e}"))?;
    let Some(block) = envelope.wake else {
        return Ok(None);
    };
    if block.host_id <= 0 {
        return Err("the host to wake is required".to_string());
    }
    if block.delay_seconds > MAX_WAKE_DELAY_SECONDS {
        return Err(format!(
            "the wait after waking must be {MAX_WAKE_DELAY_SECONDS} seconds or less"
        ));
    }
    Ok(Some(WakePlan {
        host_id: block.host_id,
        delay_seconds: block.delay_seconds,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{InterfaceStatus, InterfaceType};

    fn host(mac: &str, broadcast: Option<&str>) -> WolHost {
        WolHost {
            id: 1,
            label: "nas".to_string(),
            mac: mac.to_string(),
            broadcast: broadcast.map(str::to_string),
            created_at: "2026-10-31T00:00:00Z".to_string(),
            updated_at: "2026-10-31T00:00:00Z".to_string(),
        }
    }

    fn iface(name: &str, address: &str, mask: &str, up: bool) -> IfaceV4 {
        let address: Ipv4Addr = address.parse().unwrap();
        let mask: Ipv4Addr = mask.parse().unwrap();
        let prefix = netmask_to_prefix(mask).expect("contiguous mask");
        IfaceV4 {
            name: name.to_string(),
            address,
            prefix,
            broadcast: directed_broadcast(address, prefix).expect("broadcast"),
            up,
        }
    }

    // ---------------------------------------------------------------------
    // MAC parsing
    // ---------------------------------------------------------------------

    #[test]
    fn a_mac_is_accepted_in_the_forms_an_admin_pastes() {
        let expected = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        for value in [
            "AA:BB:CC:DD:EE:FF",
            "aa:bb:cc:dd:ee:ff",
            "AA-BB-CC-DD-EE-FF",
            "aabb.ccdd.eeff",
            "aabbccddeeff",
            " aa:bb:cc:dd:ee:ff ",
        ] {
            assert_eq!(parse_mac(value).unwrap(), expected, "{value}");
        }
        // …and always comes back canonical.
        assert_eq!(format_mac(&expected), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn a_mac_that_is_not_one_is_refused() {
        // Wrong length, non-hex, all-zero, and the broadcast address.
        assert!(parse_mac("").is_err());
        assert!(parse_mac("AA:BB:CC:DD:EE").is_err());
        assert!(parse_mac("AA:BB:CC:DD:EE:FF:00").is_err());
        assert!(parse_mac("GG:BB:CC:DD:EE:FF").is_err());
        assert!(parse_mac("00:00:00:00:00:00").is_err());
        assert!(parse_mac("FF:FF:FF:FF:FF:FF").is_err());
        // 01:… is multicast (the low bit of the first octet), 02:… is a
        // locally-administered unicast address and must be accepted.
        assert!(parse_mac("01:00:5E:00:00:01").is_err());
        assert!(parse_mac("02:00:00:00:00:01").is_ok());
    }

    // ---------------------------------------------------------------------
    // The packet
    // ---------------------------------------------------------------------

    #[test]
    fn the_magic_packet_is_six_ff_bytes_then_the_mac_sixteen_times() {
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let packet = magic_packet(&mac);
        assert_eq!(packet.len(), MAGIC_PACKET_LEN);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        for repeat in 0..16 {
            let start = 6 + repeat * 6;
            assert_eq!(&packet[start..start + 6], &mac, "repeat {repeat}");
        }
        // Byte 6 is the MAC, not another 0xFF: a 6-byte tail of the prefix
        // would make a 108-byte packet some NICs ignore.
        assert_eq!(packet[6], 0x00);
    }

    #[test]
    fn the_packet_of_a_locally_administered_mac_carries_it_verbatim() {
        let mac = parse_mac("02:00:5e:10:00:01").unwrap();
        let packet = magic_packet(&mac);
        assert_eq!(&packet[6..12], &[0x02, 0x00, 0x5E, 0x10, 0x00, 0x01]);
    }

    // ---------------------------------------------------------------------
    // Subnets
    // ---------------------------------------------------------------------

    #[test]
    fn a_contiguous_netmask_maps_to_its_prefix() {
        assert_eq!(
            netmask_to_prefix("255.255.255.0".parse().unwrap()),
            Some(24)
        );
        assert_eq!(netmask_to_prefix("255.255.0.0".parse().unwrap()), Some(16));
        assert_eq!(
            netmask_to_prefix("255.255.255.252".parse().unwrap()),
            Some(30)
        );
        assert_eq!(netmask_to_prefix("0.0.0.0".parse().unwrap()), Some(0));
        assert_eq!(
            netmask_to_prefix("255.255.255.255".parse().unwrap()),
            Some(32)
        );
        // Not contiguous: a config typo must not be rounded into a subnet.
        assert_eq!(netmask_to_prefix("255.0.255.0".parse().unwrap()), None);
        assert_eq!(netmask_to_prefix("255.255.255.1".parse().unwrap()), None);
    }

    #[test]
    fn a_prefix_maps_back_to_its_netmask() {
        assert_eq!(
            prefix_to_netmask(24),
            Some("255.255.255.0".parse().unwrap())
        );
        assert_eq!(prefix_to_netmask(0), Some("0.0.0.0".parse().unwrap()));
        assert_eq!(
            prefix_to_netmask(32),
            Some("255.255.255.255".parse().unwrap())
        );
        assert_eq!(prefix_to_netmask(33), None);
    }

    #[test]
    fn the_directed_broadcast_of_a_subnet_is_the_all_ones_host_part() {
        let ip: Ipv4Addr = "192.168.1.42".parse().unwrap();
        assert_eq!(
            directed_broadcast(ip, 24),
            Some("192.168.1.255".parse().unwrap())
        );
        assert_eq!(
            directed_broadcast("10.0.0.1".parse().unwrap(), 8),
            Some("10.255.255.255".parse().unwrap())
        );
        // A /32 has no broadcast, and a /0 would be the whole internet.
        assert_eq!(directed_broadcast(ip, 32), None);
        assert_eq!(directed_broadcast(ip, 0), Some(Ipv4Addr::BROADCAST));
        assert_eq!(directed_broadcast(ip, 40), None);
    }

    #[test]
    fn same_subnet_compares_only_the_network_part() {
        let a: Ipv4Addr = "192.168.1.10".parse().unwrap();
        let b: Ipv4Addr = "192.168.1.200".parse().unwrap();
        let c: Ipv4Addr = "192.168.2.10".parse().unwrap();
        assert!(same_subnet(a, b, 24));
        assert!(!same_subnet(a, c, 24));
        // …and a /16 does contain both.
        assert!(same_subnet(a, c, 16));
        // An impossible prefix is never "same".
        assert!(!same_subnet(a, b, 33));
    }

    // ---------------------------------------------------------------------
    // Interface selection
    // ---------------------------------------------------------------------

    #[test]
    fn the_ipv4_inventory_skips_loopback_and_addressless_interfaces() {
        let interfaces = vec![
            NetworkInterface {
                name: "lo".to_string(),
                iface_type: InterfaceType::Loopback,
                mac: None,
                status: InterfaceStatus::Up,
                ipv4: Some("127.0.0.1".to_string()),
                ipv4_netmask: Some("255.0.0.0".to_string()),
                ipv6: None,
                ipv6_prefix: None,
                ipv6_addresses: vec![],
                mtu: None,
                speed_mbps: None,
                duplex: None,
                rx_bytes: 0,
                tx_bytes: 0,
                driver: None,
            },
            NetworkInterface {
                name: "eth0".to_string(),
                iface_type: InterfaceType::Ethernet,
                mac: Some("aa:bb:cc:dd:ee:ff".to_string()),
                status: InterfaceStatus::Up,
                ipv4: Some("192.168.1.5".to_string()),
                ipv4_netmask: Some("255.255.255.0".to_string()),
                ipv6: None,
                ipv6_prefix: None,
                ipv6_addresses: vec![],
                mtu: Some(1500),
                speed_mbps: Some(1000),
                duplex: Some("full".to_string()),
                rx_bytes: 0,
                tx_bytes: 0,
                driver: None,
            },
            NetworkInterface {
                name: "eth1".to_string(),
                iface_type: InterfaceType::Ethernet,
                mac: None,
                status: InterfaceStatus::Down,
                ipv4: None,
                ipv4_netmask: None,
                ipv6: None,
                ipv6_prefix: None,
                ipv6_addresses: vec![],
                mtu: None,
                speed_mbps: None,
                duplex: None,
                rx_bytes: 0,
                tx_bytes: 0,
                driver: None,
            },
        ];
        let ifaces = ipv4_interfaces(&interfaces);
        assert_eq!(ifaces.len(), 1, "only eth0 carries an address");
        assert_eq!(ifaces[0].name, "eth0");
        assert_eq!(ifaces[0].prefix, 24);
        assert_eq!(ifaces[0].broadcast, Ipv4Addr::new(192, 168, 1, 255));
        assert!(ifaces[0].up);
    }

    #[test]
    fn the_interface_owning_the_target_subnet_is_the_one_used() {
        let ifaces = vec![
            iface("eth0", "192.168.1.5", "255.255.255.0", true),
            iface("eth1", "10.0.0.5", "255.255.255.0", true),
        ];
        let chosen = select_interface(&ifaces, Some("10.0.0.77".parse().unwrap())).unwrap();
        assert_eq!(chosen.name, "eth1");
        assert_eq!(chosen.broadcast, Ipv4Addr::new(10, 0, 0, 255));

        // No target → the first up interface.
        assert_eq!(select_interface(&ifaces, None).unwrap().name, "eth0");

        // A target on neither subnet → nothing (the caller falls back).
        assert!(select_interface(&ifaces, Some("172.16.0.1".parse().unwrap())).is_none());
    }

    #[test]
    fn a_down_interface_is_never_chosen() {
        let ifaces = vec![
            iface("eth0", "192.168.1.5", "255.255.255.0", false),
            iface("eth1", "192.168.1.6", "255.255.255.0", true),
        ];
        assert_eq!(select_interface(&ifaces, None).unwrap().name, "eth1");
        assert_eq!(
            select_interface(&ifaces, Some("192.168.1.99".parse().unwrap()))
                .unwrap()
                .name,
            "eth1"
        );
        let all_down = vec![iface("eth0", "192.168.1.5", "255.255.255.0", false)];
        assert!(select_interface(&all_down, None).is_none());
    }

    #[test]
    fn the_destination_prefers_the_override_then_the_directed_broadcast() {
        let ifaces = vec![iface("eth0", "192.168.1.5", "255.255.255.0", true)];

        // An explicit destination wins and names no interface.
        let destination =
            destination_for(&host("AA:BB:CC:DD:EE:FF", Some("192.168.9.255")), &ifaces).unwrap();
        assert_eq!(
            destination.address,
            "192.168.9.255".parse::<IpAddr>().unwrap()
        );
        assert_eq!(destination.interface, None);

        // Otherwise the selected interface's directed broadcast.
        let destination = destination_for(&host("AA:BB:CC:DD:EE:FF", None), &ifaces).unwrap();
        assert_eq!(
            destination.address,
            "192.168.1.255".parse::<IpAddr>().unwrap()
        );
        assert_eq!(destination.interface.as_deref(), Some("eth0"));

        // No usable interface at all → the limited broadcast, still honest
        // about not knowing which interface carried it.
        let destination = destination_for(&host("AA:BB:CC:DD:EE:FF", None), &[]).unwrap();
        assert_eq!(destination.address, IpAddr::V4(Ipv4Addr::BROADCAST));
        assert_eq!(destination.interface, None);

        // A name in the override field is refused: it would be resolved at
        // wake time, between a click and a packet.
        assert!(destination_for(&host("AA:BB:CC:DD:EE:FF", Some("nas.local")), &ifaces).is_err());
    }

    // ---------------------------------------------------------------------
    // Validation and the report's wording
    // ---------------------------------------------------------------------

    #[test]
    fn a_label_is_trimmed_and_bounded() {
        assert_eq!(
            validate_label("  Living room PC  ").unwrap(),
            "Living room PC"
        );
        assert!(validate_label("   ").is_err());
        assert!(validate_label(&"x".repeat(65)).is_err());
        assert!(validate_label("bad\u{0}label").is_err());
        assert!(validate_label(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn a_destination_is_an_ip_literal_or_nothing() {
        assert_eq!(validate_broadcast("").unwrap(), None);
        assert_eq!(validate_broadcast("  ").unwrap(), None);
        assert_eq!(
            validate_broadcast(" 192.168.1.255 ").unwrap(),
            Some("192.168.1.255".to_string())
        );
        assert!(validate_broadcast("fe80::1").unwrap().is_some());
        // A host name would mean a DNS lookup at wake time.
        assert!(validate_broadcast("nas.local").is_err());
        assert!(validate_broadcast("192.168.1.255/24").is_err());
    }

    #[test]
    fn the_report_never_claims_the_host_woke() {
        let report = WakeReport {
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            destination: "192.168.1.255".to_string(),
            interface: Some("eth0".to_string()),
            ports: WOL_PORTS.to_vec(),
        };
        let message = report.message();
        assert!(message.contains("192.168.1.255"));
        assert!(message.contains("eth0"));
        assert!(
            message.contains("only confirms it left"),
            "an unacknowledged datagram cannot confirm a wake: {message}"
        );
        assert!(!message.to_lowercase().contains("woken"));

        let without_interface = WakeReport {
            interface: None,
            ..report
        };
        assert!(without_interface.message().contains("192.168.1.255"));
    }

    // ---------------------------------------------------------------------
    // Scheduled-task "wake first"
    // ---------------------------------------------------------------------

    #[test]
    fn a_wake_block_is_parsed_out_of_any_task_payload() {
        let plan = parse_wake_plan(
            r#"{"type":"snapshot","pool":"tank","recursive":false,"wake":{"host_id":3,"delay_seconds":90}}"#,
        )
        .unwrap()
        .expect("a plan");
        assert_eq!(plan.host_id, 3);
        assert_eq!(plan.delay_seconds, 90);

        // A payload without the block is simply a task that does not wake
        // anything — the shape every existing task already has.
        assert!(
            parse_wake_plan(r#"{"type":"command","command":"/bin/true"}"#)
                .unwrap()
                .is_none()
        );
        // …including a payload that is a bare object with no `type` at all
        // (the executor re-validates the task separately).
        assert!(parse_wake_plan("{}").unwrap().is_none());
        // A missing delay means "send, then run at once".
        let immediate =
            parse_wake_plan(r#"{"type":"power","action":"reboot","wake":{"host_id":1}}"#)
                .unwrap()
                .unwrap();
        assert_eq!(immediate.delay_seconds, 0);
    }

    #[test]
    fn a_bad_wake_block_is_refused_rather_than_ignored() {
        // A typo in the wait must not silently become "wake and run now".
        assert!(parse_wake_plan(
            r#"{"type":"power","action":"reboot","wake":{"host_id":1,"delay":90}}"#
        )
        .is_err());
        assert!(
            parse_wake_plan(r#"{"type":"power","action":"reboot","wake":{"host_id":0}}"#).is_err()
        );
        assert!(parse_wake_plan(
            r#"{"type":"power","action":"reboot","wake":{"host_id":1,"delay_seconds":601}}"#
        )
        .is_err());
        assert!(parse_wake_plan(
            r#"{"type":"power","action":"reboot","wake":{"host_id":1,"delay_seconds":600}}"#
        )
        .is_ok());
        assert!(parse_wake_plan("not json").is_err());
    }
}
