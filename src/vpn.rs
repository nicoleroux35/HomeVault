/* =============================================================================
 * File          : vpn.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.1
 *
 * Description:
 *   VPN engine service for WireGuard (server + client) and OpenVPN
 *   (client-only, imported profiles): capability detection, peer
 *   provisioning (keys, config files), tunnel start/stop, and live
 *   status — always reading the host state (wg show) as the source of truth.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   handlers/vpn.rs (see docs/guides/vpn.md).
 * =============================================================================
 */

use crate::utils::process::{run_timed, run_timed_detached, run_timed_with_stdin};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Deadlines (ROADMAP X.2)
//
// Every host command of the VPN module used to be a bare `Command::output`
// (or a hand-rolled `.spawn` + `wait_with_output`). A `wg`/`wg-quick`
// call blocked on a missing kernel module, an `openvpn` daemonize that never
// returns or a `pgrep` on a huge process table pinned the calling thread with
// no ceiling — and the VPN screen polls the server status and the peer list.
// Each value below is a ceiling for the slowest honest case, not an estimate.
// ---------------------------------------------------------------------------

/// A read-only VPN probe: `wg show`/`--version`/`genkey`/`genpsk`, `pgrep`,
/// `ip route`, `sudo cat`/`sudo test`.
pub const VPN_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// A VPN mutation: `wg set`, `wg-quick up`, `sudo chmod`, `systemctl enable`,
/// `sudo kill`, and the `openvpn --daemon` parent. Root operations can wait
/// on the sudo/PAM stack or on the kernel, so this is longer than a probe
/// while staying finite.
pub const VPN_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Run one host command under a deadline, keeping the reason it failed.
///
/// The VPN module is synchronous by design (the async entry points call it
/// through `spawn_blocking`), so this is the blocking helper. The timeout is
/// a parameter rather than a constant baked into each call site so the kill
/// path is unit-testable, exactly like `disk::bounded` and
/// `cache::run_command`.
fn run_command(cmd: &mut Command, timeout: Duration) -> Result<Output, String> {
    run_timed(cmd, timeout)
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnConnection {
    pub id: String,
    pub name: String,
    pub vpn_type: VpnType,
    pub status: VpnStatus,
    pub interface: Option<String>,
    pub local_ip: Option<String>,
    pub remote_ip: Option<String>,
    pub public_key: Option<String>,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VpnType {
    WireGuard,
    OpenVPN,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VpnStatus {
    Connected,
    Disconnected,
    Connecting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenVPNConfig {
    pub name: String,
    pub config_file: String,
    pub auth_user_pass: Option<(String, String)>,
}

// ---------------------------------------------------------------------------
// WireGuard Management — single always-on server + provisioned peers
// ---------------------------------------------------------------------------
// Design: rather than the generic "create an outbound tunnel to some other
// peer" model OpenVPN still uses below, HomeVault runs one persistent
// WireGuard *server* interface (`wg0`) and each "connection" the user
// creates is really a new roadwarrior peer (a phone, a laptop) provisioned
// against it. This is the only way to get the requested "just type a name,
// scan a QR code" experience: the app already knows the server's own
// keypair, subnet, port and public endpoint, so a peer only needs a
// freshly generated keypair, the next free address, and nothing the user
// has to supply by hand. The server is provisioned lazily, on the very
// first peer creation, so there is no separate "set up the VPN server"
// step either.

pub const WG_INTERFACE: &str = "wg0";
pub const WG_SERVER_PORT: u16 = 51820;
pub const WG_SERVER_ADDRESS: &str = "10.8.0.1/24";
/// Peers are handed out sequential addresses in this /24, starting at .2
/// (.1 is the server itself).
const WG_SUBNET_OCTETS: &str = "10.8.0";

fn wg_server_config_path() -> String {
    format!("/etc/wireguard/{}.conf", WG_INTERFACE)
}

/// True if the WireGuard server config exists, checked via `sudo test -e`
/// rather than `Path::exists()`. `/etc/wireguard` is `0750 homevault:homevault`
/// (see `src/setup.rs::prepare_vpn_directories`) — `Path::exists()` can't
/// even traverse into it for a process that isn't running as exactly that
/// user (e.g. this binary launched manually rather than via the packaged
/// systemd unit), and silently reports "doesn't exist" instead of "can't
/// tell", which would make `ensure_wireguard_server` below regenerate a
/// brand new server keypair — invalidating every already-provisioned peer —
/// on every single call instead of just the first one.
fn wg_config_exists() -> bool {
    let mut cmd = Command::new("sudo");
    cmd.args(["test", "-e", &wg_server_config_path()]);
    run_command(&mut cmd, VPN_PROBE_TIMEOUT)
        .map(|s| s.status.success())
        .unwrap_or(false)
}

/// Reads the WireGuard server config via `sudo cat`. The file is
/// deliberately `0600` (it holds the server's own private key), so unlike
/// the other root-owned configs this codebase reads directly (`/etc/fstab`,
/// `smb.conf`, ...) — which stay default-readable — even the `homevault`
/// service account can't read this one back with a plain
/// `std::fs::read_to_string` once it's been written.
fn read_wg_config() -> Result<String, String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["cat", &wg_server_config_path()]);
    let output = run_command(&mut cmd, VPN_PROBE_TIMEOUT).map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Failed to read WireGuard server config: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// chmod's the WireGuard server config via `sudo chmod` — `sudo tee` always
/// creates its output owned by root, so a plain `std::fs::set_permissions`
/// on it fails the same way a direct write would.
fn chmod_wg_config(mode: &str) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["chmod", mode, &wg_server_config_path()]);
    let output = run_command(&mut cmd, VPN_WRITE_TIMEOUT).map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Failed to set WireGuard server config permissions: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Check if WireGuard is available
pub fn wireguard_available() -> bool {
    let mut cmd = Command::new("wg");
    cmd.arg("--version");
    run_command(&mut cmd, VPN_PROBE_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detects the interface carrying this host's default route, so the
/// server's NAT rule (below) knows which interface to masquerade behind —
/// without it, a peer with a full-tunnel `AllowedIPs = 0.0.0.0/0` could
/// reach the server itself but never get real internet access through it.
fn detect_egress_interface() -> Option<String> {
    let mut cmd = Command::new("ip");
    cmd.args(["route", "get", "1.1.1.1"]);
    let output = run_command(&mut cmd, VPN_PROBE_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut tokens = text.split_whitespace();
    while let Some(tok) = tokens.next() {
        if tok == "dev" {
            return tokens.next().map(|s| s.to_string());
        }
    }
    None
}

/// True once `wg0` genuinely exists as a live network interface, not just a
/// config file on disk.
fn is_wireguard_server_running() -> bool {
    let mut cmd = Command::new("sudo");
    cmd.args(["wg", "show", WG_INTERFACE]);
    run_command(&mut cmd, VPN_PROBE_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Brings up `wg0` via `wg-quick`, which also runs the config's PostUp
/// NAT/forwarding rules. Treats "already exists" (the interface was already
/// up from an earlier call) as success rather than an error.
fn start_wireguard_server() -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["wg-quick", "up", WG_INTERFACE]);
    let output = run_command(&mut cmd, VPN_WRITE_TIMEOUT).map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already exists") {
            return Ok(());
        }
        return Err(format!("Failed to start the WireGuard server: {}", stderr));
    }

    Ok(())
}

/// Enables the packaged `wg-quick@wg0` systemd unit so the server survives
/// a reboot with no separate admin step — best-effort, matching this
/// feature's "everything automatic" goal. Requires the scoped sudoers grant
/// added alongside this feature in `src/setup.rs::configure_sudoers`.
fn enable_wireguard_server_on_boot() {
    let mut cmd = Command::new("sudo");
    cmd.args(["systemctl", "enable", &format!("wg-quick@{}", WG_INTERFACE)]);
    let _ = run_command(&mut cmd, VPN_WRITE_TIMEOUT);
}

/// Idempotently ensures the WireGuard server exists, is configured, and is
/// running — generating its keypair and writing its config on the very
/// first call, starting it if the config exists but the interface isn't up
/// yet. Every peer operation calls this first.
pub async fn ensure_wireguard_server() -> Result<WireGuardServerInfo, String> {
    if !wg_config_exists() {
        let (private_key, _public_key) = generate_wireguard_keypair()?;
        let egress = detect_egress_interface().unwrap_or_else(|| "eth0".to_string());

        let config_content = format!(
            "[Interface]\n\
             Address = {address}\n\
             ListenPort = {port}\n\
             PrivateKey = {private_key}\n\
             PostUp = sysctl -w net.ipv4.ip_forward=1; iptables -t nat -A POSTROUTING -o {egress} -j MASQUERADE; iptables -A FORWARD -i {iface} -j ACCEPT; iptables -A FORWARD -o {iface} -j ACCEPT\n\
             PostDown = iptables -t nat -D POSTROUTING -o {egress} -j MASQUERADE; iptables -D FORWARD -i {iface} -j ACCEPT; iptables -D FORWARD -o {iface} -j ACCEPT\n\n",
            address = WG_SERVER_ADDRESS,
            port = WG_SERVER_PORT,
            private_key = private_key,
            egress = egress,
            iface = WG_INTERFACE,
        );

        // The config holds the server's own private key, so it's written
        // and locked down via sudo the same way every other root-owned
        // config write in this codebase is — see `read_wg_config`'s doc
        // comment for why this one additionally needs sudo to read back,
        // unlike `/etc/fstab`/`smb.conf`.
        crate::shares::write_file_as_root(&wg_server_config_path(), &config_content)
            .map_err(|e| format!("Failed to write WireGuard server config: {}", e))?;
        chmod_wg_config("600")?;

        start_wireguard_server()?;
        enable_wireguard_server_on_boot();

        // Best-effort: if ufw is installed, open the port now so a later
        // firewall enable doesn't silently strand every VPN peer with no
        // way to reach the handshake port. Never fatal — ufw may not be
        // installed at all, which is fine.
        let _ = crate::firewall::add_rule(
            &WG_SERVER_PORT.to_string(),
            "udp",
            "allow",
            None,
            "homevault-wireguard",
        )
        .await;
    } else if !is_wireguard_server_running() {
        start_wireguard_server()?;
    }

    // Verify the interface actually came up before telling the caller
    // "running": wg-quick has been observed to fail silently (missing
    // privileges, script errors in PostUp), and the whole provisioning flow
    // would otherwise report a server that can't accept any peer.
    if !is_wireguard_server_running() {
        // Small grace: wg-quick returns after wiring the interface, give
        // the kernel a beat before declaring failure.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if !is_wireguard_server_running() {
            return Err(
                "The WireGuard interface did not come up (check the sudoers grants and the server config)"
                    .to_string(),
            );
        }
    }

    get_wireguard_server_status().await
}

/// Live server info: its public key (derived from the persisted private
/// key, never stored directly), listen port, detected public endpoint, and
/// whether the interface is actually up right now.
#[derive(Debug, Clone, Serialize)]
pub struct WireGuardServerInfo {
    pub public_key: String,
    pub listen_port: u16,
    pub endpoint_host: Option<String>,
    /// Where `endpoint_host` came from: `manual`, `ddns`, `public_ip`, `unset`.
    pub endpoint_source: String,
    /// A sentence when the endpoint is unset or only reachable on the LAN.
    pub endpoint_warning: Option<String>,
    pub running: bool,
}

/// The admin's endpoint override. Empty means "automatic" (DDNS, then the
/// detected public IP), so a home user never has to type an address unless
/// the automatic value is wrong.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WgEndpointConfig {
    #[serde(default)]
    pub host: String,
}

fn endpoint_config_path() -> std::path::PathBuf {
    crate::config::paths::data_dir().join("wg_endpoint.json")
}

pub fn load_endpoint_config() -> WgEndpointConfig {
    std::fs::read_to_string(endpoint_config_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<WgEndpointConfig>(&raw).ok())
        .unwrap_or_default()
}

/// Persists the override atomically. An empty host clears it.
pub fn save_endpoint_config(host: &str) -> Result<(), String> {
    let path = endpoint_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_vec_pretty(&WgEndpointConfig {
        host: host.trim().to_string(),
    })
    .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// A public endpoint is either a bare DNS name (same rules as DDNS) or an
/// IPv4/IPv6 address. Schemes, ports and paths are refused so the value can
/// be spliced into `Endpoint = <host>:<port>` without ambiguity.
pub fn validate_endpoint_host(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.parse::<std::net::Ipv4Addr>().is_ok() || value.parse::<std::net::Ipv6Addr>().is_ok() {
        return Ok(value.to_string());
    }
    crate::ddns::validate_hostname(value)
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1])
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// The sentence shown under the endpoint. `None` when the value is a DNS name
/// or a public address (we cannot prove reachability, but there is nothing
/// obviously wrong to warn about).
pub fn endpoint_warning(host: Option<&str>) -> Option<String> {
    let Some(host) = host.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(
            "No public endpoint is known yet: set a dynamic DNS name or enter your public IP so phones and laptops can reach this server."
                .to_string(),
        );
    };
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_private_ip(ip) {
            return Some(
                "This endpoint is a private, LAN or CGNAT address: it only works inside your network. Use a dynamic DNS name or a public IP for remote access."
                    .to_string(),
            );
        }
    }
    None
}

/// Resolves the endpoint used in peer configs, in order: manual override,
/// the configured DDNS hostname, the detected public IP.
pub async fn resolve_endpoint() -> (Option<String>, String) {
    let manual = load_endpoint_config().host;
    if !manual.trim().is_empty() {
        return (Some(manual), "manual".to_string());
    }
    let ddns = crate::ddns::load_settings();
    if ddns.enabled && !ddns.hostname.trim().is_empty() {
        return (Some(ddns.hostname), "ddns".to_string());
    }
    match crate::network::discover_public_ip().await {
        Some(ip) => (Some(ip.to_string()), "public_ip".to_string()),
        None => (None, "unset".to_string()),
    }
}

pub async fn get_wireguard_server_status() -> Result<WireGuardServerInfo, String> {
    let public_key = read_server_public_key()?;
    let running = is_wireguard_server_running();
    let (endpoint_host, endpoint_source) = resolve_endpoint().await;
    let endpoint_warning = endpoint_warning(endpoint_host.as_deref());

    Ok(WireGuardServerInfo {
        public_key,
        listen_port: WG_SERVER_PORT,
        endpoint_host,
        endpoint_source,
        endpoint_warning,
        running,
    })
}

fn read_server_public_key() -> Result<String, String> {
    let config_content = read_wg_config()?;
    let private_key = config_content
        .lines()
        .find_map(|l| l.trim().strip_prefix("PrivateKey = "))
        .ok_or_else(|| "WireGuard server config has no PrivateKey".to_string())?;
    derive_public_key(private_key)
}

fn derive_public_key(private_key: &str) -> Result<String, String> {
    let mut cmd = Command::new("wg");
    cmd.arg("pubkey");
    // `wg pubkey` reads the private key on stdin; `run_timed_with_stdin`
    // feeds it from its own thread and bounds the wait, so a `wg` blocked on
    // a missing kernel module cannot park the caller.
    let output = run_timed_with_stdin(&mut cmd, VPN_PROBE_TIMEOUT, private_key.as_bytes())
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err("Failed to derive public key".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Generate a WireGuard key pair
pub fn generate_wireguard_keypair() -> Result<(String, String), String> {
    let mut cmd = Command::new("wg");
    cmd.arg("genkey");
    let private_output = run_command(&mut cmd, VPN_PROBE_TIMEOUT).map_err(|e| e.to_string())?;

    if !private_output.status.success() {
        return Err("Failed to generate private key".to_string());
    }

    let private_key = String::from_utf8_lossy(&private_output.stdout)
        .trim()
        .to_string();
    let public_key = derive_public_key(&private_key)?;

    Ok((private_key, public_key))
}

/// Generate a preshared key — an extra symmetric secret layered on top of
/// each peer's own keypair, standard WireGuard practice for defense in
/// depth against a future break of the asymmetric handshake.
pub fn generate_wireguard_psk() -> Result<String, String> {
    let mut cmd = Command::new("wg");
    cmd.arg("genpsk");
    let output = run_command(&mut cmd, VPN_PROBE_TIMEOUT).map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err("Failed to generate preshared key".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Picks the next unused address in the peer subnet, given the bare
/// addresses (no CIDR suffix, e.g. "10.8.0.2") already assigned to existing
/// peers.
pub fn next_available_peer_ip(used_ips: &[String]) -> Result<String, String> {
    for host in 2..255u32 {
        let candidate = format!("{}.{}", WG_SUBNET_OCTETS, host);
        if !used_ips.iter().any(|ip| ip == &candidate) {
            return Ok(candidate);
        }
    }
    Err("The WireGuard peer subnet is full (253 devices already provisioned)".to_string())
}

/// A freshly provisioned peer: its keys and the complete client config text.
/// The private key and preshared key are now persisted in the DB so the
/// config/QR code can be re-displayed later, not shown only once at creation.
#[derive(Debug, Clone, Serialize)]
pub struct WireGuardPeerProvisioned {
    pub public_key: String,
    pub private_key: String,
    pub preshared_key: String,
    pub peer_ip: String,
    pub client_config: String,
}

/// Provisions a new peer against the server: generates its keypair and a
/// preshared key, applies it live via `wg set` (so it can connect
/// immediately, without restarting `wg0` and dropping every other already-
/// connected peer), then persists the same peer block to the config file so
/// it survives a reboot.
pub async fn create_wireguard_peer(peer_ip: &str) -> Result<WireGuardPeerProvisioned, String> {
    let server = ensure_wireguard_server().await?;

    let (peer_private_key, peer_public_key) = generate_wireguard_keypair()?;
    let preshared_key = generate_wireguard_psk()?;

    let mut cmd = Command::new("sudo");
    cmd.args([
        "wg",
        "set",
        WG_INTERFACE,
        "peer",
        &peer_public_key,
        "preshared-key",
        "/dev/stdin",
        "allowed-ips",
        &format!("{}/32", peer_ip),
    ]);
    // The preshared key goes in on stdin; `run_timed_with_stdin` feeds it from
    // its own thread and bounds the wait, so a `wg set` that never returns
    // cannot park the peer-provisioning job.
    let set_output = run_timed_with_stdin(
        &mut cmd,
        VPN_WRITE_TIMEOUT,
        format!("{}\n", preshared_key).as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    if !set_output.status.success() {
        let stderr = String::from_utf8_lossy(&set_output.stderr);
        return Err(format!("Failed to add peer: {}", stderr));
    }

    append_peer_to_server_config(&peer_public_key, &preshared_key, peer_ip)?;

    let client_config = build_client_config(&peer_private_key, peer_ip, &server, &preshared_key);

    Ok(WireGuardPeerProvisioned {
        public_key: peer_public_key,
        private_key: peer_private_key,
        preshared_key,
        peer_ip: peer_ip.to_string(),
        client_config,
    })
}

/// Rebuild the full client config for an existing peer whose keys are stored
/// in the DB. This allows the QR code / download to be re-shown later, not
/// only at creation time.
pub async fn get_wireguard_peer_config(
    peer_ip: &str,
    private_key: &str,
    preshared_key: &str,
) -> Result<String, String> {
    let server = get_wireguard_server_status().await?;
    Ok(build_client_config(
        private_key,
        peer_ip,
        &server,
        preshared_key,
    ))
}

fn append_peer_to_server_config(
    public_key: &str,
    preshared_key: &str,
    peer_ip: &str,
) -> Result<(), String> {
    let block = format!(
        "[Peer]\nPublicKey = {}\nPresharedKey = {}\nAllowedIPs = {}/32\n\n",
        public_key, preshared_key, peer_ip
    );
    crate::shares::append_file_as_root(&wg_server_config_path(), &block)
        .map_err(|e| format!("Failed to update WireGuard server config: {}", e))
}

fn build_client_config(
    private_key: &str,
    peer_ip: &str,
    server: &WireGuardServerInfo,
    preshared_key: &str,
) -> String {
    let endpoint_host = server
        .endpoint_host
        .clone()
        .unwrap_or_else(|| "YOUR-SERVER-IP".to_string());
    format!(
        "[Interface]\n\
         PrivateKey = {private_key}\n\
         Address = {peer_ip}/32\n\
         DNS = 1.1.1.1\n\n\
         [Peer]\n\
         PublicKey = {server_public_key}\n\
         PresharedKey = {preshared_key}\n\
         Endpoint = {endpoint_host}:{port}\n\
         AllowedIPs = 0.0.0.0/0, ::/0\n\
         PersistentKeepalive = 25\n",
        private_key = private_key,
        peer_ip = peer_ip,
        server_public_key = server.public_key,
        preshared_key = preshared_key,
        endpoint_host = endpoint_host,
        port = server.listen_port,
    )
}

/// Revokes a peer: removes it from the live interface immediately, then
/// from the persisted config so it doesn't come back on the next reboot.
pub fn revoke_wireguard_peer(public_key: &str) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["wg", "set", WG_INTERFACE, "peer", public_key, "remove"]);
    let _ = run_command(&mut cmd, VPN_WRITE_TIMEOUT);

    remove_peer_from_server_config(public_key)
}

fn remove_peer_from_server_config(public_key: &str) -> Result<(), String> {
    let content = read_wg_config()?;

    let marker = format!("PublicKey = {}", public_key);
    let kept: Vec<&str> = content
        .split("\n\n")
        .filter(|block| !block.lines().any(|l| l.trim() == marker))
        .collect();

    crate::shares::write_file_as_root(&wg_server_config_path(), &kept.join("\n\n"))
        .map_err(|e| format!("Failed to update WireGuard server config: {}", e))
}

/// Live per-peer status from the kernel (`wg show wg0 dump`), keyed by
/// public key so the caller can join it against the DB-persisted peer
/// registry (name, creation date) for display.
#[derive(Debug, Clone, Serialize)]
pub struct WireGuardPeerLiveStatus {
    pub public_key: String,
    /// Unix epoch seconds of the last handshake, or 0 if never connected.
    pub latest_handshake: u64,
    pub transfer_rx: u64,
    pub transfer_tx: u64,
}

pub fn list_wireguard_peer_live_status() -> Vec<WireGuardPeerLiveStatus> {
    let mut cmd = Command::new("sudo");
    cmd.args(["wg", "show", WG_INTERFACE, "dump"]);
    let output = run_command(&mut cmd, VPN_PROBE_TIMEOUT);

    let mut statuses = Vec::new();
    if let Ok(output) = output {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            // First line describes the interface itself, not a peer.
            for line in text.lines().skip(1) {
                let fields: Vec<&str> = line.split('\t').collect();
                if fields.len() >= 7 {
                    statuses.push(WireGuardPeerLiveStatus {
                        public_key: fields[0].to_string(),
                        latest_handshake: fields[4].parse().unwrap_or(0),
                        transfer_rx: fields[5].parse().unwrap_or(0),
                        transfer_tx: fields[6].parse().unwrap_or(0),
                    });
                }
            }
        }
    }
    statuses
}

// ---------------------------------------------------------------------------
// OpenVPN Management
// ---------------------------------------------------------------------------

pub(crate) const OPENVPN_CONFIG_DIR: &str = "/etc/openvpn";

/// How long `start_openvpn_connection` waits for the client to report an
/// established tunnel before declaring the start failed and tearing the
/// daemon back down. A genuine connection completes in 1–3 s; the grace is
/// intentionally generous so a slow TLS negotiation on a far server still
/// succeeds.
const OPENVPN_START_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// Poll cadence while waiting for the connect marker in the log.
const OPENVPN_START_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// The log line a client OpenVPN daemon writes exactly once when the tunnel
/// is up (verified empirically against OpenVPN 2.6: the `--status` file of a
/// *client* instance only carries statistics counters, never the connection
/// state, so the log file is the reliable signal).
const OPENVPN_CONNECTED_MARKER: &str = "Initialization Sequence Completed";

/// Directives that let an uploaded `.ovpn` profile execute external commands
/// once launched via `openvpn --config`. The daemon runs as root, so a
/// profile carrying any of these is a privilege escalation — rejected at
/// save time with an explicit message (`script-security` is checked
/// separately: level 1, the default, runs no scripts and stays allowed).
const OPENVPN_EXEC_DIRECTIVES: &[&str] = &[
    "up",
    "down",
    "down-pre",
    "route-up",
    "ipchange",
    "learn-address",
    "client-connect",
    "client-disconnect",
    "auth-user-pass-verify",
    "tls-verify",
    "askpass",
];

/// Rejects a `.ovpn` upload that would execute external commands as root.
/// Only whole lines starting with the directive count (values such as
/// `remote vpn.example.com` never match), and matching is case-insensitive.
pub fn validate_openvpn_config_content(content: &str) -> Result<(), String> {
    for raw_line in content.lines() {
        let line = raw_line.trim();
        let Some(word) = line.split_whitespace().next() else {
            continue;
        };
        let word = word.to_ascii_lowercase();
        if word == "script-security" {
            let level: u32 = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            if level > 1 {
                return Err(format!(
                    "The OpenVPN configuration enables script-security {level}: the daemon would run external scripts as root. Level 1 (no scripts) is allowed."
                ));
            }
            continue;
        }
        if OPENVPN_EXEC_DIRECTIVES.contains(&word.as_str()) {
            return Err(format!(
                "The OpenVPN configuration uses the '{}' directive, which opens external commands: on HomeVault the OpenVPN daemon runs as root, so such a profile is a security risk and is refused.",
                word
            ));
        }
    }
    Ok(())
}

/// True when a client-mode log tail shows the tunnel established.
pub fn openvpn_connected_from_log(text: &str) -> bool {
    text.lines().any(|l| l.contains(OPENVPN_CONNECTED_MARKER))
}

/// Pulls the most informative error line out of a failed client log, for
/// the user-visible "why didn't it connect" answer. Empty text → None.
pub fn openvpn_fatal_from_log(text: &str) -> Option<String> {
    let fatal = text
        .lines()
        .rev()
        .find(|l| l.contains("Exiting due to fatal error") || l.contains("Options error"))
        .map(|l| l.to_string());
    fatal.or_else(|| {
        // No fatal exit (the daemon may be retrying forever): surface the
        // last transport-level error instead, e.g. a network-prefix-free
        // read failure line.
        text.lines()
            .rev()
            .find(|l| l.contains("Connection refused") || l.contains("Connection timed out"))
            .map(|l| l.to_string())
    })
}

/// Check if OpenVPN is available
pub fn openvpn_available() -> bool {
    let mut cmd = Command::new("openvpn");
    cmd.arg("--version");
    run_command(&mut cmd, VPN_PROBE_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// List all OpenVPN connections
pub fn list_openvpn_connections() -> Vec<VpnConnection> {
    let mut connections = Vec::new();

    // Check running OpenVPN processes
    let mut cmd = Command::new("pgrep");
    cmd.args(["-a", "openvpn"]);
    let output = run_command(&mut cmd, VPN_PROBE_TIMEOUT);

    if let Ok(output) = output {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                // Extract config file name from command line
                if let Some(config_name) = extract_openvpn_config_name(line) {
                    connections.push(VpnConnection {
                        id: config_name.clone(),
                        name: config_name.clone(),
                        vpn_type: VpnType::OpenVPN,
                        status: VpnStatus::Connected,
                        interface: Some("tun0".to_string()),
                        local_ip: None,
                        remote_ip: None,
                        public_key: None,
                        endpoint: None,
                        allowed_ips: Vec::new(),
                        created_at: None,
                    });
                }
            }
        }
    }

    connections
}

fn extract_openvpn_config_name(line: &str) -> Option<String> {
    // Look for --config parameter or .ovpn file
    for part in line.split_whitespace() {
        if part.ends_with(".ovpn") || part.ends_with(".conf") {
            if let Some(name) = Path::new(part).file_stem() {
                return name.to_str().map(|s| s.to_string());
            }
        }
    }
    None
}

/// Collect saved (possibly stopped) connection names from a config directory.
/// Returns names that are NOT already present in `active`, so we don't
/// duplicate connections that are currently running.
fn saved_config_names(config_dir: &str, extension: &str, active: &[VpnConnection]) -> Vec<String> {
    let mut names = Vec::new();
    let entries = match std::fs::read_dir(config_dir) {
        Ok(e) => e,
        Err(_) => return names,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(extension) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let stem = stem.to_string();
            if !active.iter().any(|c| c.id == stem) {
                names.push(stem);
            }
        }
    }

    names
}

/// True when an OpenVPN daemon is already running for `config_path` (a
/// second start would bind-fight the first one).
pub(crate) fn is_openvpn_running_for_config(config_path: &str) -> bool {
    let mut cmd = Command::new("pgrep");
    cmd.args(["-f", &format!("--config {config_path}")]);
    run_command(&mut cmd, VPN_PROBE_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Credential file for a connection's `--auth-user-pass`. Lives in the
/// homevault-owned config dir (NOT /run — the daemon reads it as root, but
/// only the service can create files there), kept while the connection runs:
/// OpenVPN re-reads it on every renegotiation, so deleting it after the
/// first connect would break later handshakes. Removed on stop/delete.
fn openvpn_auth_file_path(name: &str) -> String {
    format!("{OPENVPN_CONFIG_DIR}/openvpn-{name}.auth")
}

/// Log file for a connection's daemon. The client `--status` file carries
/// no connection state (verified empirically), so the log is what the start
/// verification polls for the `Initialization Sequence Completed` marker.
fn openvpn_log_file_path(name: &str) -> String {
    format!("{OPENVPN_CONFIG_DIR}/{name}.log")
}

/// Start an OpenVPN connection — and VERIFY that it actually connected.
///
/// `openvpn --daemon` forks and reports success even when the TLS handshake
/// never completes (wrong credentials, unreachable endpoint, bad profile),
/// which used to answer "started" for a connection that is silently
/// retrying forever. The daemon is started with a per-connection log file
/// and `--cd` (it chdir()s to / when it daemonizes, breaking every relative
/// path inside the profile), then the log is polled for the connect marker
/// within [`OPENVPN_START_GRACE`]. On timeout the daemon is killed
/// (rollback) and the last transport error from the log is surfaced.
pub fn start_openvpn_connection(config: &OpenVPNConfig) -> Result<(), String> {
    let config_path = &config.config_file;

    if !Path::new(config_path).exists() {
        return Err(format!("Config file not found: {}", config_path));
    }
    if is_openvpn_running_for_config(config_path) {
        // Idempotent: a running daemon is a fine outcome for "start".
        tracing::warn!("OpenVPN connection '{}' already running", config.name);
        return Ok(());
    }
    // Fresh log every start, or the success marker of a previous run would
    // satisfy the verification instantly.
    let log_path = openvpn_log_file_path(&config.name);
    let _ = std::fs::remove_file(&log_path);

    // `openvpn --daemon` must run as root to create the tunnel.
    // The homevault service runs unprivileged, so it delegates via sudo.
    let mut cmd = Command::new("sudo");
    cmd.arg("openvpn")
        .args(["--config", config_path, "--cd", OPENVPN_CONFIG_DIR])
        .args(["--log", &log_path, "--daemon"]);

    // Add auth credentials if provided
    if let Some((user, pass)) = &config.auth_user_pass {
        let auth_file = openvpn_auth_file_path(&config.name);
        std::fs::write(&auth_file, format!("{}\n{}\n", user, pass))
            .map_err(|e| format!("Failed to write auth file: {}", e))?;
        // Restrict permissions so only root can read the credentials file
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&auth_file, perms) {
                tracing::warn!("Failed to set auth file permissions: {}", e);
            }
        }
        cmd.args(["--auth-user-pass", &auth_file]);
    }

    // Run with detached stdio, bounded: `output()` would hold stdout/stderr
    // pipes open until every writer closes them — and the forked daemon could
    // keep them alive, parking this call forever (which is why the shared
    // `run_timed` cannot be used here: its drain threads never see EOF).
    // Everything openvpn has to say lands in the `--log` file anyway
    // (verified empirically: parse-time `Options error`s are written there
    // too). `run_timed_detached` waits for the daemonizing parent under
    // `VPN_WRITE_TIMEOUT` and kills it if it never exits.
    let status = run_timed_detached(&mut cmd, VPN_WRITE_TIMEOUT).map_err(|e| e.to_string())?;

    if !status.success() {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let detail = openvpn_fatal_from_log(&text)
            .map(|l| openvpn_log_error_summary(&l))
            .unwrap_or_else(|| "daemon exited before connecting".to_string());
        return Err(format!("Failed to start OpenVPN: {}", detail));
    }

    // ── Verify the tunnel actually came up ────────────────────────────────
    let deadline = std::time::Instant::now() + OPENVPN_START_GRACE;
    loop {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if openvpn_connected_from_log(&text) {
            return Ok(());
        }
        if let Some(fatal) = openvpn_fatal_from_log(&text) {
            // The daemon already died — no need to wait out the grace. Keep
            // the summary before stop removes the log file.
            let detail = openvpn_log_error_summary(&fatal);
            let _ = stop_openvpn_connection(&config.name);
            return Err(format!(
                "OpenVPN connection '{}' failed to start: {}",
                config.name, detail
            ));
        }
        if std::time::Instant::now() >= deadline {
            // Rollback: never leave a half-connected daemon retrying
            // forever behind a "success" the user already saw.
            let detail = openvpn_fatal_from_log(&text)
                .map(|l| format!(": {}", openvpn_log_error_summary(&l)))
                .unwrap_or_default();
            let _ = stop_openvpn_connection(&config.name);
            return Err(format!(
                "OpenVPN connection '{}' did not connect within {} s{} (check the endpoint and credentials in the profile)",
                config.name,
                OPENVPN_START_GRACE.as_secs(),
                detail
            ));
        }
        std::thread::sleep(OPENVPN_START_POLL);
    }
}

/// Trims the log timestamp off an error line so the API message reads as a
/// sentence, not a syslog fragment. Parse-time errors (e.g.
/// `Options error: --cert fails …`) carry no timestamp at all and must pass
/// through unchanged.
fn openvpn_log_error_summary(line: &str) -> String {
    let mut parts = line.splitn(3, ' ');
    let date = parts.next().unwrap_or("");
    let time = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    // `%Y-%m-%d %H:%M:%S message` — the date is `YYYY-MM-DD` and the time
    // contains at least one ':'.
    if date.len() == 10
        && date.as_bytes().get(4) == Some(&b'-')
        && date.as_bytes().get(7) == Some(&b'-')
        && time.contains(':')
        && !rest.is_empty()
    {
        rest.trim().to_string()
    } else {
        line.trim().to_string()
    }
}

/// Stop an OpenVPN connection — killing the daemon and removing its log and
/// credential files.
pub fn stop_openvpn_connection(name: &str) -> Result<(), String> {
    // Find OpenVPN process with this config
    let config_path = format!("{}/{}.ovpn", OPENVPN_CONFIG_DIR, name);
    let mut cmd = Command::new("pgrep");
    cmd.args(["-f", &format!("--config {config_path}")]);
    let output = run_command(&mut cmd, VPN_PROBE_TIMEOUT).map_err(|e| e.to_string())?;

    if output.status.success() {
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid in pids.lines() {
            let pid = pid.trim();
            // Validate PID is numeric to prevent command injection
            if !pid.chars().all(|c| c.is_ascii_digit()) || pid.is_empty() {
                continue;
            }
            // The openvpn process runs as root, so the unprivileged
            // homevault service must delegate the kill to sudo. The signal is
            // explicit (`-TERM`) because the sudoers grant is pinned to it —
            // see `configure_sudoers()`.
            let mut cmd = Command::new("sudo");
            cmd.args(["kill", "-TERM", pid]);
            let _ = run_command(&mut cmd, VPN_WRITE_TIMEOUT);
        }
    }

    // Best-effort cleanup of this connection's own files. The .auth file
    // holds the credentials in clear text; keeping it after stop is a
    // leftover secret on disk.
    let _ = std::fs::remove_file(openvpn_auth_file_path(name));
    let _ = std::fs::remove_file(openvpn_log_file_path(name));

    Ok(())
}

/// Upload and save OpenVPN config — after content validation. The daemon
/// launched from this file runs as root, so a profile with script/exec
/// hooks is a privilege escalation and is refused here, before anything is
/// written to disk.
pub fn save_openvpn_config(name: &str, config_content: &str) -> Result<String, String> {
    validate_openvpn_config_content(config_content)?;

    let config_dir = OPENVPN_CONFIG_DIR;
    std::fs::create_dir_all(config_dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let config_path = format!("{}/{}.ovpn", config_dir, name);
    std::fs::write(&config_path, config_content)
        .map_err(|e| format!("Failed to write config: {}", e))?;

    Ok(config_path)
}

/// Delete an OpenVPN connection
pub fn delete_openvpn_connection(name: &str) -> Result<(), String> {
    // Stop if running
    let _ = stop_openvpn_connection(name);

    // Delete config file
    let config_path = format!("{}/{}.ovpn", OPENVPN_CONFIG_DIR, name);
    if Path::new(&config_path).exists() {
        std::fs::remove_file(&config_path)
            .map_err(|e| format!("Failed to delete config: {}", e))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// WireGuard client tunnels (outbound provider configurations)
// ---------------------------------------------------------------------------
// The WireGuard implementation above is a *server*: HomeVault owns `wg0` and
// provisions roadwarrior peers. This second, independent path lets the
// appliance itself be a WireGuard client of a remote provider or concentrator.
// Imported configurations are parsed and re-rendered from an allowlist — a
// provider's `PostUp`/`PreUp` directive would otherwise run a command as root,
// exactly like the OpenVPN profile deny list below.

/// Prefix of every client interface. The prefix is what the sudoers grants
/// pin (`hvc-*`), so a client can never name `wg0` or an unrelated interface.
pub const WG_CLIENT_PREFIX: &str = "hvc-";
/// The `/etc/wireguard` directory, shared with the server config.
pub const WG_CLIENT_CONFIG_DIR: &str = "/etc/wireguard";
/// How long the boot job waits for the network before reconnecting clients.
pub const WG_CLIENT_BOOT_DELAY: Duration = Duration::from_secs(20);
/// A client is "connected" when its last handshake is younger than this.
const WG_CLIENT_CONNECTED_WINDOW: u64 = 180;
/// The `wg-quick up` verification grace and poll cadence.
const WG_CLIENT_START_GRACE: Duration = Duration::from_secs(15);
const WG_CLIENT_START_POLL: Duration = Duration::from_millis(300);
/// Store key -> `{DATA_DIRECTORY}/data/wireguard_clients.json`.
const WG_CLIENT_STORE_KEY: &str = "wireguard_clients";
/// AES-GCM associated data: a sealed config cannot be moved to another row.
fn client_secret_scope(id: &str) -> String {
    format!("wireguard-client:{id}")
}

/// One `[Peer]` block of a client configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgClientPeer {
    pub public_key: String,
    pub preshared_key: Option<String>,
    pub allowed_ips: Vec<String>,
    pub endpoint: Option<String>,
    pub persistent_keepalive: Option<u16>,
}

/// A parsed WireGuard client configuration. Only the directives HomeVault can
/// render safely are kept; everything else is refused by the parser.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WgClientConfig {
    pub private_key: String,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub mtu: Option<u16>,
    pub listen_port: Option<u16>,
    pub peers: Vec<WgClientPeer>,
}

/// Why an import, start, stop or delete was refused. The handler maps each
/// variant to the status the API contract requires.
#[derive(Debug)]
pub enum WireGuardClientError {
    /// The uploaded configuration is invalid (400).
    Validation(String),
    /// A client with the same display name already exists (409).
    AlreadyExists(String),
    /// No client with that id (404).
    NotFound,
    /// `wireguard-tools` is not installed (503).
    Unavailable(String),
    /// A host command or file operation failed (500).
    Host(String),
}

impl std::fmt::Display for WireGuardClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(m) | Self::AlreadyExists(m) | Self::Unavailable(m) | Self::Host(m) => {
                write!(f, "{m}")
            }
            Self::NotFound => write!(f, "WireGuard client not found"),
        }
    }
}

impl std::error::Error for WireGuardClientError {}

impl From<String> for WireGuardClientError {
    fn from(value: String) -> Self {
        Self::Validation(value)
    }
}

/// The persisted definition of one imported client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGuardClientRecord {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub endpoint: Option<String>,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub allowed_ips: Vec<String>,
    pub peer_count: usize,
    pub kill_switch: bool,
    #[serde(default = "default_true")]
    pub autostart: bool,
    pub created_at: String,
    pub created_by: String,
    /// The rendered configuration, sealed with the appliance key (`hv1:`).
    /// The private key never appears in an API payload.
    pub config_sealed: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WireGuardClientStore {
    #[serde(default)]
    clients: Vec<WireGuardClientRecord>,
}

/// What the list endpoint may expose: metadata + live state, never the config.
#[derive(Debug, Clone, Serialize)]
pub struct WireGuardClientView {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub endpoint: Option<String>,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub allowed_ips: Vec<String>,
    pub peer_count: usize,
    pub kill_switch: bool,
    pub autostart: bool,
    pub created_at: String,
    pub interface_up: bool,
    pub connected: bool,
    pub latest_handshake: Option<u64>,
    pub transfer_rx: u64,
    pub transfer_tx: u64,
}

/// Live kernel state of one client interface.
#[derive(Debug, Clone, Default)]
pub struct WireGuardClientLiveStatus {
    pub interface_up: bool,
    pub connected: bool,
    pub latest_handshake: Option<u64>,
    pub transfer_rx: u64,
    pub transfer_tx: u64,
}

/// The exact host commands used for one interface. Pure so the argv is
/// asserted by unit tests instead of being inferred from a running host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireGuardClientCommands {
    pub up: Vec<String>,
    pub down: Vec<String>,
    pub show_dump: Vec<String>,
}

pub fn wireguard_client_commands(interface: &str) -> WireGuardClientCommands {
    WireGuardClientCommands {
        up: vec![
            "wg-quick".to_string(),
            "up".to_string(),
            interface.to_string(),
        ],
        down: vec![
            "wg-quick".to_string(),
            "down".to_string(),
            interface.to_string(),
        ],
        show_dump: vec![
            "wg".to_string(),
            "show".to_string(),
            interface.to_string(),
            "dump".to_string(),
        ],
    }
}

/// The host actions an import performs when it has to roll back. `down` comes
/// first (a half-created interface must not outlive the record), then the
/// rendered config file is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireGuardClientRollbackStep {
    Down,
    RemoveConfig,
}

pub fn wireguard_client_import_rollback_plan() -> [WireGuardClientRollbackStep; 2] {
    [
        WireGuardClientRollbackStep::Down,
        WireGuardClientRollbackStep::RemoveConfig,
    ]
}

/// Deterministic interface name from the client id: `hvc-` + 10 characters,
/// 14 bytes total (Linux caps interface names at 15). The user-supplied name
/// is display-only, so it can never choose `wg0` or another interface.
pub fn wireguard_client_interface(id: &str) -> String {
    let compact: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(10)
        .collect();
    format!("{WG_CLIENT_PREFIX}{compact}")
}

pub fn wireguard_client_config_path(interface: &str) -> std::path::PathBuf {
    Path::new(WG_CLIENT_CONFIG_DIR).join(format!("{interface}.conf"))
}

/// Refuse a stored interface name that is not one HomeVault generated. The
/// store is on disk and can be restored or edited, so it is never trusted as
/// a path/argv fragment: a tampered record must not reach `/etc/wireguard`
/// or `sudo wg-quick`.
fn validate_client_interface(interface: &str) -> Result<(), String> {
    if interface.len() <= 15
        && interface.starts_with(WG_CLIENT_PREFIX)
        && interface
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        Ok(())
    } else {
        Err(format!(
            "stored WireGuard interface name '{interface}' is invalid"
        ))
    }
}

/// Accumulates one `[Peer]` block while the parser walks the file.
#[derive(Default)]
struct PeerBuilder {
    public_key: Option<String>,
    preshared_key: Option<String>,
    allowed_ips: Option<Vec<String>>,
    endpoint: Option<String>,
    persistent_keepalive: Option<u16>,
}

/// Parse a WireGuard client configuration from an allowlist.
///
/// Refused with a named message: `PreUp`/`PostUp`/`PreDown`/`PostDown` (the
/// config is executed as root by `wg-quick`), `SaveConfig` (HomeVault owns the
/// file), `Table`/`FwMark` (HomeVault derives routes from `AllowedIPs`) and
/// every unknown directive. Values are validated before they are re-rendered,
/// so an import can never smuggle a second line into the generated file.
pub fn parse_wireguard_client_config(content: &str) -> Result<WgClientConfig, String> {
    let mut section: Option<&str> = None;
    let mut interface_seen = false;
    let mut private_key: Option<String> = None;
    let mut addresses: Option<Vec<String>> = None;
    let mut dns: Vec<String> = Vec::new();
    let mut mtu: Option<u16> = None;
    let mut listen_port: Option<u16> = None;
    let mut peers: Vec<WgClientPeer> = Vec::new();
    let mut peer: Option<PeerBuilder> = None;
    let mut seen_interface: HashSet<&'static str> = HashSet::new();
    let mut seen_peer: HashSet<&'static str> = HashSet::new();

    for (index, raw) in content.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line_no = index + 1;

        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(format!("line {line_no}: malformed section header"));
            }
            let name = line[1..line.len() - 1].trim().to_ascii_lowercase();
            if let Some(builder) = peer.take() {
                peers.push(builder.finish(line_no)?);
            }
            match name.as_str() {
                "interface" => {
                    if interface_seen {
                        return Err("only one [Interface] section is supported".to_string());
                    }
                    interface_seen = true;
                    section = Some("interface");
                }
                "peer" => {
                    if !interface_seen {
                        return Err(format!("line {line_no}: [Peer] appears before [Interface]"));
                    }
                    section = Some("peer");
                    peer = Some(PeerBuilder::default());
                    seen_peer.clear();
                }
                other => {
                    return Err(format!(
                        "line {line_no}: unsupported section '[{other}]' (only [Interface] and [Peer] are accepted)"
                    ));
                }
            }
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {line_no}: expected 'Key = value', found '{line}'"))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            return Err(format!("line {line_no}: empty key or value"));
        }
        let directive = key.to_ascii_lowercase();

        match section {
            Some("interface") => {
                if directive != "dns" && !seen_interface.insert(static_directive(&directive)) {
                    return Err(format!(
                        "line {line_no}: duplicate directive '{key}' in [Interface]"
                    ));
                }
                match directive.as_str() {
                    "privatekey" => {
                        private_key = Some(parse_wireguard_key("PrivateKey", value)?);
                    }
                    "address" => {
                        addresses = Some(parse_cidr_list("Address", value)?);
                    }
                    "dns" => {
                        let parsed = parse_dns_list(value)?;
                        if dns.is_empty() {
                            dns = parsed;
                        } else {
                            dns.extend(parsed);
                        }
                    }
                    "mtu" => {
                        let parsed: u32 = value
                            .parse()
                            .map_err(|_| format!("line {line_no}: MTU must be a whole number"))?;
                        if !(576..=65535).contains(&parsed) {
                            return Err(format!(
                                "line {line_no}: MTU must be between 576 and 65535"
                            ));
                        }
                        mtu = Some(parsed as u16);
                    }
                    "listenport" => {
                        let parsed: u32 = value.parse().map_err(|_| {
                            format!("line {line_no}: ListenPort must be a whole number")
                        })?;
                        if !(1..=65535).contains(&parsed) {
                            return Err(format!(
                                "line {line_no}: ListenPort must be between 1 and 65535"
                            ));
                        }
                        listen_port = Some(parsed as u16);
                    }
                    "preup" | "postup" | "predown" | "postdown" => {
                        return Err(format!(
                            "line {line_no}: '{key}' is refused — wg-quick runs it as root, so an imported configuration may not execute commands"
                        ));
                    }
                    "saveconfig" => {
                        return Err(format!(
                            "line {line_no}: 'SaveConfig' is not supported — HomeVault owns the generated configuration file"
                        ));
                    }
                    "table" | "fwmark" => {
                        return Err(format!(
                            "line {line_no}: '{key}' is not supported — routes come from AllowedIPs"
                        ));
                    }
                    _ => {
                        return Err(format!(
                            "line {line_no}: unsupported directive '{key}' in [Interface]"
                        ));
                    }
                }
            }
            Some("peer") => {
                let builder = peer.as_mut().ok_or_else(|| {
                    format!("line {line_no}: peer directive outside a [Peer] section")
                })?;
                if !seen_peer.insert(static_directive(&directive)) {
                    return Err(format!(
                        "line {line_no}: duplicate directive '{key}' in [Peer]"
                    ));
                }
                match directive.as_str() {
                    "publickey" => {
                        builder.public_key = Some(parse_wireguard_key("PublicKey", value)?);
                    }
                    "presharedkey" => {
                        builder.preshared_key = Some(parse_wireguard_key("PresharedKey", value)?);
                    }
                    "allowedips" => {
                        builder.allowed_ips = Some(parse_cidr_list("AllowedIPs", value)?);
                    }
                    "endpoint" => {
                        builder.endpoint = Some(parse_wireguard_endpoint(value, line_no)?);
                    }
                    "persistentkeepalive" => {
                        let parsed: u32 = value.parse().map_err(|_| {
                            format!("line {line_no}: PersistentKeepalive must be a whole number")
                        })?;
                        if parsed > 65535 {
                            return Err(format!(
                                "line {line_no}: PersistentKeepalive must be between 0 and 65535"
                            ));
                        }
                        builder.persistent_keepalive = Some(parsed as u16);
                    }
                    _ => {
                        return Err(format!(
                            "line {line_no}: unsupported directive '{key}' in [Peer]"
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "line {line_no}: directive '{key}' appears outside a section"
                ));
            }
        }
    }

    if let Some(builder) = peer.take() {
        peers.push(builder.finish(content.lines().count())?);
    }

    if !interface_seen {
        return Err("the configuration has no [Interface] section".to_string());
    }
    let private_key =
        private_key.ok_or_else(|| "the [Interface] section has no PrivateKey".to_string())?;
    let addresses = addresses
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "the [Interface] section has no Address".to_string())?;
    if peers.is_empty() {
        return Err("the configuration has no [Peer] section".to_string());
    }

    Ok(WgClientConfig {
        private_key,
        addresses,
        dns,
        mtu,
        listen_port,
        peers,
    })
}

impl WgClientConfig {
    /// The first peer endpoint, which is what the UI shows on the card.
    pub fn endpoint(&self) -> Option<String> {
        self.peers.iter().find_map(|p| p.endpoint.clone())
    }

    /// Every AllowedIPs network across peers, de-duplicated in order.
    pub fn allowed_ips(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for peer in &self.peers {
            for network in &peer.allowed_ips {
                if seen.insert(network.clone()) {
                    out.push(network.clone());
                }
            }
        }
        out
    }
}

impl PeerBuilder {
    fn finish(self, line_no: usize) -> Result<WgClientPeer, String> {
        let public_key = self
            .public_key
            .ok_or_else(|| format!("line {line_no}: [Peer] has no PublicKey"))?;
        let allowed_ips = self
            .allowed_ips
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("line {line_no}: [Peer] has no AllowedIPs"))?;
        Ok(WgClientPeer {
            public_key,
            preshared_key: self.preshared_key,
            allowed_ips,
            endpoint: self.endpoint,
            persistent_keepalive: self.persistent_keepalive,
        })
    }
}

/// `HashSet<&'static str>` needs a `'static` key. The directive set is small
/// and fixed, so map it to the canonical spelling instead of leaking the
/// caller's string.
fn static_directive(directive: &str) -> &'static str {
    match directive {
        "privatekey" => "privatekey",
        "address" => "address",
        "dns" => "dns",
        "mtu" => "mtu",
        "listenport" => "listenport",
        "publickey" => "publickey",
        "presharedkey" => "presharedkey",
        "allowedips" => "allowedips",
        "endpoint" => "endpoint",
        "persistentkeepalive" => "persistentkeepalive",
        _ => "unknown",
    }
}

fn parse_wireguard_key(field: &str, value: &str) -> Result<String, String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| format!("{field} is not a valid WireGuard key (base64 expected)"))?;
    if decoded.len() != 32 {
        return Err(format!("{field} must decode to a 32-byte WireGuard key"));
    }
    Ok(value.to_string())
}

fn parse_cidr_list(field: &str, value: &str) -> Result<Vec<String>, String> {
    let mut networks = Vec::new();
    for raw in value.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        // WireGuard requires a prefix on `Address`; `IpNetwork` would accept a
        // bare host address as an implicit /32 (or /128), which the client
        // would then install as a host route instead of the intended network.
        if !raw.contains('/') {
            return Err(format!(
                "{field} contains '{raw}' without a prefix (use CIDR, for example 10.2.0.2/32)"
            ));
        }
        raw.parse::<ipnetwork::IpNetwork>()
            .map_err(|_| format!("{field} contains an invalid network '{raw}'"))?;
        networks.push(raw.to_string());
    }
    if networks.is_empty() {
        return Err(format!("{field} must contain at least one network"));
    }
    Ok(networks)
}

fn parse_dns_list(value: &str) -> Result<Vec<String>, String> {
    let mut servers = Vec::new();
    for raw in value.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        raw.parse::<std::net::IpAddr>().map_err(|_| {
            format!("DNS contains an invalid address '{raw}' (only IP addresses are accepted)")
        })?;
        servers.push(raw.to_string());
    }
    if servers.is_empty() {
        return Err("DNS must contain at least one IP address".to_string());
    }
    Ok(servers)
}

fn parse_wireguard_endpoint(value: &str, line_no: usize) -> Result<String, String> {
    let (host, port_text) = if let Some(rest) = value.strip_prefix('[') {
        let (host, rest) = rest
            .split_once(']')
            .ok_or_else(|| format!("line {line_no}: Endpoint has an unterminated IPv6 address"))?;
        let port = rest
            .strip_prefix(':')
            .ok_or_else(|| format!("line {line_no}: Endpoint must be '[IPv6]:port'"))?;
        (host, port)
    } else {
        let (host, port) = value
            .rsplit_once(':')
            .ok_or_else(|| format!("line {line_no}: Endpoint must be 'host:port'"))?;
        if host.contains(':') {
            return Err(format!(
                "line {line_no}: an IPv6 Endpoint must be written '[address]:port'"
            ));
        }
        (host, port)
    };

    if host.is_empty() || port_text.is_empty() {
        return Err(format!(
            "line {line_no}: Endpoint is missing its host or port"
        ));
    }
    if host.parse::<std::net::IpAddr>().is_err() && !is_valid_hostname(host) {
        return Err(format!(
            "line {line_no}: Endpoint host '{host}' is not a valid IP address or hostname"
        ));
    }
    let port: u16 = port_text
        .parse()
        .map_err(|_| format!("line {line_no}: Endpoint port must be between 1 and 65535"))?;
    if port == 0 {
        return Err(format!(
            "line {line_no}: Endpoint port must be between 1 and 65535"
        ));
    }

    Ok(if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    })
}

fn is_valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Render the canonical configuration. Only values that passed the parser are
/// written, so a value can never inject a second directive.
pub fn render_wireguard_client_config(config: &WgClientConfig, kill_switch: bool) -> String {
    let mut out = String::new();
    out.push_str("[Interface]\n");
    out.push_str(&format!("PrivateKey = {}\n", config.private_key));
    out.push_str(&format!("Address = {}\n", config.addresses.join(", ")));
    if !config.dns.is_empty() {
        out.push_str(&format!("DNS = {}\n", config.dns.join(", ")));
    }
    if let Some(mtu) = config.mtu {
        out.push_str(&format!("MTU = {mtu}\n"));
    }
    if let Some(port) = config.listen_port {
        out.push_str(&format!("ListenPort = {port}\n"));
    }
    if kill_switch {
        out.push_str(KILL_SWITCH_POST_UP);
        out.push_str(KILL_SWITCH_PRE_DOWN);
    }

    for peer in &config.peers {
        out.push_str("\n[Peer]\n");
        out.push_str(&format!("PublicKey = {}\n", peer.public_key));
        if let Some(psk) = &peer.preshared_key {
            out.push_str(&format!("PresharedKey = {psk}\n"));
        }
        out.push_str(&format!("AllowedIPs = {}\n", peer.allowed_ips.join(", ")));
        if let Some(endpoint) = &peer.endpoint {
            out.push_str(&format!("Endpoint = {endpoint}\n"));
        }
        if let Some(keepalive) = peer.persistent_keepalive {
            out.push_str(&format!("PersistentKeepalive = {keepalive}\n"));
        }
    }
    out
}

/// The fixed kill-switch rules. `%i` is expanded by `wg-quick`; `ip6tables`
/// is skipped when absent so an IPv4-only host can still use the option.
const KILL_SWITCH_POST_UP: &str = "PostUp = iptables -I OUTPUT ! -o %i -m mark ! --mark $(wg show %i fwmark) -m addrtype ! --dst-type LOCAL -j REJECT; if command -v ip6tables >/dev/null 2>&1; then ip6tables -I OUTPUT ! -o %i -m mark ! --mark $(wg show %i fwmark) -m addrtype ! --dst-type LOCAL -j REJECT; fi\n";
const KILL_SWITCH_PRE_DOWN: &str = "PreDown = iptables -D OUTPUT ! -o %i -m mark ! --mark $(wg show %i fwmark) -m addrtype ! --dst-type LOCAL -j REJECT; if command -v ip6tables >/dev/null 2>&1; then ip6tables -D OUTPUT ! -o %i -m mark ! --mark $(wg show %i fwmark) -m addrtype ! --dst-type LOCAL -j REJECT; fi\n";

fn client_store() -> crate::storage::json::JsonStorage {
    crate::storage::json::JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

fn load_client_store() -> Result<WireGuardClientStore, String> {
    match client_store().load::<WireGuardClientStore>(WG_CLIENT_STORE_KEY) {
        Ok(Some(store)) => Ok(store),
        Ok(None) => Ok(WireGuardClientStore::default()),
        Err(e) => Err(format!("Could not read the WireGuard client store: {e}")),
    }
}

fn save_client_store(store: &WireGuardClientStore) -> Result<(), String> {
    client_store()
        .save(WG_CLIENT_STORE_KEY, store)
        .map_err(|e| format!("Could not save the WireGuard client store: {e}"))
}

fn run_client_command(args: &[String], timeout: Duration) -> Result<Output, String> {
    let mut cmd = Command::new("sudo");
    cmd.args(args);
    run_command(&mut cmd, timeout)
}

fn is_client_interface_up(interface: &str) -> bool {
    if validate_client_interface(interface).is_err() {
        return false;
    }
    let commands = wireguard_client_commands(interface);
    run_client_command(&commands.show_dump, VPN_PROBE_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_client_config_file(interface: &str, content: &str) -> Result<(), String> {
    validate_client_interface(interface)?;
    let path = wireguard_client_config_path(interface);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, content)
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Could not restrict {}: {e}", path.display()))?;
    }
    Ok(())
}

fn remove_client_config_file(interface: &str) -> Result<(), String> {
    validate_client_interface(interface)?;
    let path = wireguard_client_config_path(interface);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not remove {}: {e}", path.display())),
    }
}

fn decrypt_client_config(record: &WireGuardClientRecord) -> Result<String, WireGuardClientError> {
    crate::storage::secret_box::open(&client_secret_scope(&record.id), &record.config_sealed)
        .map_err(|e| {
            WireGuardClientError::Host(format!(
                "The stored configuration for '{}' cannot be read ({e}). Delete the client and import the .conf again.",
                record.name
            ))
        })
}

fn stop_client_interface(interface: &str) -> Result<(), String> {
    validate_client_interface(interface)?;
    let commands = wireguard_client_commands(interface);
    let output = run_client_command(&commands.down, VPN_WRITE_TIMEOUT)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    // Already down is a fine outcome for an idempotent stop/delete.
    if stderr.contains("not a wireguard interface")
        || stderr.contains("cannot find device")
        || stderr.contains("is not up")
    {
        return Ok(());
    }
    Err(format!(
        "Could not stop WireGuard interface '{}': {}",
        interface,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn start_client_record(record: &WireGuardClientRecord) -> Result<(), WireGuardClientError> {
    if !wireguard_available() {
        return Err(WireGuardClientError::Unavailable(
            "WireGuard is not installed (install the 'wireguard-tools' package).".to_string(),
        ));
    }

    // Rewrite the file from the sealed store before every start: a restored
    // data directory or a hand-edited root file must not diverge from the
    // record the UI shows.
    let content = decrypt_client_config(record)?;
    write_client_config_file(&record.interface, &content).map_err(WireGuardClientError::Host)?;

    if is_client_interface_up(&record.interface) {
        return Ok(());
    }

    let commands = wireguard_client_commands(&record.interface);
    let output =
        run_client_command(&commands.up, VPN_WRITE_TIMEOUT).map_err(WireGuardClientError::Host)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.to_lowercase().contains("already exists") {
            // A partially-created interface must not be left behind.
            let _ = stop_client_interface(&record.interface);
            return Err(WireGuardClientError::Host(format!(
                "Could not start WireGuard client '{}': {}",
                record.name,
                stderr.trim()
            )));
        }
    }

    // `wg-quick up` can return success while PostUp/resolvconf failed and the
    // interface never appeared. Verify the kernel, then roll back on timeout.
    let deadline = Instant::now() + WG_CLIENT_START_GRACE;
    loop {
        if is_client_interface_up(&record.interface) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = stop_client_interface(&record.interface);
            return Err(WireGuardClientError::Host(format!(
                "WireGuard client '{}' did not come up within {} s (check DNS/resolvconf and the kill-switch rules)",
                record.name,
                WG_CLIENT_START_GRACE.as_secs()
            )));
        }
        std::thread::sleep(WG_CLIENT_START_POLL);
    }
}

/// Live state from `wg show <iface> dump`. The interface line is skipped; a
/// handshake younger than [`WG_CLIENT_CONNECTED_WINDOW`] means connected.
pub fn wireguard_client_live_status(interface: &str) -> WireGuardClientLiveStatus {
    if validate_client_interface(interface).is_err() {
        return WireGuardClientLiveStatus::default();
    }
    let commands = wireguard_client_commands(interface);
    let output = match run_client_command(&commands.show_dump, VPN_PROBE_TIMEOUT) {
        Ok(output) if output.status.success() => output,
        _ => return WireGuardClientLiveStatus::default(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let mut latest = 0u64;
    let mut transfer_rx = 0u64;
    let mut transfer_tx = 0u64;
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() >= 7 {
            let handshake: u64 = fields[4].parse().unwrap_or(0);
            latest = latest.max(handshake);
            transfer_rx = transfer_rx.saturating_add(fields[5].parse().unwrap_or(0));
            transfer_tx = transfer_tx.saturating_add(fields[6].parse().unwrap_or(0));
        }
    }
    let connected = latest > 0 && now.saturating_sub(latest) < WG_CLIENT_CONNECTED_WINDOW;
    WireGuardClientLiveStatus {
        interface_up: true,
        connected,
        latest_handshake: (latest > 0).then_some(latest),
        transfer_rx,
        transfer_tx,
    }
}

fn client_view(record: &WireGuardClientRecord) -> WireGuardClientView {
    let live = wireguard_client_live_status(&record.interface);
    WireGuardClientView {
        id: record.id.clone(),
        name: record.name.clone(),
        interface: record.interface.clone(),
        endpoint: record.endpoint.clone(),
        addresses: record.addresses.clone(),
        dns: record.dns.clone(),
        allowed_ips: record.allowed_ips.clone(),
        peer_count: record.peer_count,
        kill_switch: record.kill_switch,
        autostart: record.autostart,
        created_at: record.created_at.clone(),
        interface_up: live.interface_up,
        connected: live.connected,
        latest_handshake: live.latest_handshake,
        transfer_rx: live.transfer_rx,
        transfer_tx: live.transfer_tx,
    }
}

/// Import (and immediately connect) a provider configuration. On any failure
/// after the file was written, the interface is brought down, the file is
/// removed and the record is not kept.
pub fn import_wireguard_client(
    name: &str,
    config_content: &str,
    kill_switch: bool,
    autostart: bool,
    created_by: &str,
) -> Result<WireGuardClientView, WireGuardClientError> {
    // Validation first: an invalid upload is a 400 even on a host without
    // `wireguard-tools`, so the caller can fix the file before installing it.
    let config =
        parse_wireguard_client_config(config_content).map_err(WireGuardClientError::Validation)?;
    if !wireguard_available() {
        return Err(WireGuardClientError::Unavailable(
            "WireGuard is not installed (install the 'wireguard-tools' package).".to_string(),
        ));
    }

    let mut store = load_client_store().map_err(WireGuardClientError::Host)?;
    if store
        .clients
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name))
    {
        return Err(WireGuardClientError::AlreadyExists(format!(
            "A WireGuard client named '{name}' already exists"
        )));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let interface = wireguard_client_interface(&id);
    if interface == WG_INTERFACE {
        // Defensive: the `hvc-` prefix can never produce `wg0`, but a
        // collision would hand an imported config the server interface.
        return Err(WireGuardClientError::Host(
            "Generated WireGuard interface name collides with the server".to_string(),
        ));
    }
    if store.clients.iter().any(|c| c.interface == interface) {
        return Err(WireGuardClientError::Host(
            "Generated WireGuard interface name already exists".to_string(),
        ));
    }

    let rendered = render_wireguard_client_config(&config, kill_switch);
    let sealed =
        crate::storage::secret_box::seal(&client_secret_scope(&id), &rendered).map_err(|e| {
            WireGuardClientError::Host(format!("Could not seal the configuration: {e}"))
        })?;

    let record = WireGuardClientRecord {
        id,
        name: name.to_string(),
        interface,
        endpoint: config.endpoint(),
        addresses: config.addresses.clone(),
        dns: config.dns.clone(),
        allowed_ips: config.allowed_ips(),
        peer_count: config.peers.len(),
        kill_switch,
        autostart,
        created_at: chrono::Utc::now().to_rfc3339(),
        created_by: created_by.to_string(),
        config_sealed: sealed,
    };

    write_client_config_file(&record.interface, &rendered).map_err(WireGuardClientError::Host)?;
    store.clients.push(record.clone());
    if let Err(e) = save_client_store(&store) {
        let _ = remove_client_config_file(&record.interface);
        return Err(WireGuardClientError::Host(e));
    }

    if let Err(e) = start_client_record(&record) {
        // Full rollback: interface, file, then record.
        for step in wireguard_client_import_rollback_plan() {
            match step {
                WireGuardClientRollbackStep::Down => {
                    let _ = stop_client_interface(&record.interface);
                }
                WireGuardClientRollbackStep::RemoveConfig => {
                    let _ = remove_client_config_file(&record.interface);
                }
            }
        }
        store.clients.retain(|c| c.id != record.id);
        let _ = save_client_store(&store);
        return Err(e);
    }

    Ok(client_view(&record))
}

pub fn list_wireguard_clients() -> Result<Vec<WireGuardClientView>, String> {
    let store = load_client_store()?;
    Ok(store.clients.iter().map(client_view).collect())
}

pub fn start_wireguard_client(id: &str) -> Result<WireGuardClientView, WireGuardClientError> {
    let store = load_client_store().map_err(WireGuardClientError::Host)?;
    let record = store
        .clients
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or(WireGuardClientError::NotFound)?;
    start_client_record(&record)?;
    Ok(client_view(&record))
}

pub fn stop_wireguard_client(id: &str) -> Result<WireGuardClientView, WireGuardClientError> {
    let store = load_client_store().map_err(WireGuardClientError::Host)?;
    let record = store
        .clients
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or(WireGuardClientError::NotFound)?;
    if !wireguard_available() {
        return Err(WireGuardClientError::Unavailable(
            "WireGuard is not installed (install the 'wireguard-tools' package).".to_string(),
        ));
    }
    stop_client_interface(&record.interface).map_err(WireGuardClientError::Host)?;
    Ok(client_view(&record))
}

pub fn delete_wireguard_client(id: &str) -> Result<String, WireGuardClientError> {
    let mut store = load_client_store().map_err(WireGuardClientError::Host)?;
    let record = store
        .clients
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or(WireGuardClientError::NotFound)?;

    if wireguard_available() {
        let _ = stop_client_interface(&record.interface);
    }
    remove_client_config_file(&record.interface).map_err(WireGuardClientError::Host)?;
    store.clients.retain(|c| c.id != id);
    save_client_store(&store).map_err(WireGuardClientError::Host)?;
    Ok(record.name)
}

pub fn set_wireguard_client_autostart(
    id: &str,
    autostart: bool,
) -> Result<WireGuardClientView, WireGuardClientError> {
    let mut store = load_client_store().map_err(WireGuardClientError::Host)?;
    let record = store
        .clients
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or(WireGuardClientError::NotFound)?;
    record.autostart = autostart;
    let view = client_view(record);
    save_client_store(&store).map_err(WireGuardClientError::Host)?;
    Ok(view)
}

/// Re-connect every client whose "start at boot" flag is on. Called once at
/// startup after [`WG_CLIENT_BOOT_DELAY`]; failures are returned as strings so
/// the caller can log each one without aborting the others.
pub fn restore_autostart_clients() -> Vec<String> {
    let store = match load_client_store() {
        Ok(store) => store,
        Err(e) => return vec![e],
    };
    if !store.clients.iter().any(|c| c.autostart) || !wireguard_available() {
        return Vec::new();
    }
    store
        .clients
        .iter()
        .filter(|c| c.autostart)
        .filter_map(|record| {
            start_client_record(record)
                .err()
                .map(|e| format!("{}: {}", record.name, e))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Unified VPN API
// ---------------------------------------------------------------------------

/// List all OpenVPN connections. WireGuard has its own dedicated
/// server+peer endpoints now (see `handle_wg_*` in `handlers/vpn.rs`) since
/// a peer isn't a "connection" in this same start/stop sense — this
/// generic listing stays OpenVPN-only.
pub fn list_all_vpn_connections() -> Vec<VpnConnection> {
    let mut connections = Vec::new();

    if openvpn_available() {
        connections.extend(list_openvpn_connections());
    }

    for name in saved_config_names(OPENVPN_CONFIG_DIR, "ovpn", &connections) {
        connections.push(VpnConnection {
            id: name.clone(),
            name: name.clone(),
            vpn_type: VpnType::OpenVPN,
            status: VpnStatus::Disconnected,
            interface: Some("tun0".to_string()),
            local_ip: None,
            remote_ip: None,
            public_key: None,
            endpoint: None,
            allowed_ips: Vec::new(),
            created_at: None,
        });
    }

    connections
}

/// Get an OpenVPN connection by ID (see `list_all_vpn_connections`'s own
/// doc comment for why WireGuard isn't part of this generic lookup).
pub fn get_vpn_connection(id: &str) -> Option<VpnConnection> {
    list_openvpn_connections()
        .into_iter()
        .find(|conn| conn.id == id)
}

/// Get system VPN capabilities
/// `Default` (nothing detected) is what a failed background task reports —
/// honest and safe: it never exposes features that can't be verified.
#[derive(Debug, Default, Serialize)]
pub struct VpnCapabilities {
    pub wireguard: bool,
    pub openvpn: bool,
}

pub fn get_vpn_capabilities() -> VpnCapabilities {
    VpnCapabilities {
        wireguard: wireguard_available(),
        openvpn: openvpn_available(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- next_available_peer_ip ------------------------------------------

    #[test]
    fn test_next_available_peer_ip_empty_list_returns_first_host() {
        assert_eq!(next_available_peer_ip(&[]).unwrap(), "10.8.0.2");
    }

    #[test]
    fn test_next_available_peer_ip_skips_used_addresses() {
        let used = vec!["10.8.0.2".to_string(), "10.8.0.3".to_string()];
        assert_eq!(next_available_peer_ip(&used).unwrap(), "10.8.0.4");
    }

    #[test]
    fn test_next_available_peer_ip_finds_gap() {
        let used = vec!["10.8.0.2".to_string(), "10.8.0.4".to_string()];
        assert_eq!(next_available_peer_ip(&used).unwrap(), "10.8.0.3");
    }

    #[test]
    fn test_next_available_peer_ip_subnet_full() {
        let used: Vec<String> = (2..255u32).map(|h| format!("10.8.0.{}", h)).collect();
        assert!(next_available_peer_ip(&used).is_err());
    }

    #[test]
    fn test_next_available_peer_ip_ignores_unrelated_strings() {
        // Garbage in `used_ips` (never expected in practice — the caller
        // always passes real peer_ip values from the DB) must not panic or
        // be mistaken for a real match.
        let used = vec!["not-an-ip".to_string()];
        assert_eq!(next_available_peer_ip(&used).unwrap(), "10.8.0.2");
    }

    // -- extract_openvpn_config_name --------------------------------------

    #[test]
    fn test_extract_openvpn_config_name_ovpn_extension() {
        let line = "openvpn --config /etc/openvpn/client/office.ovpn --daemon";
        assert_eq!(
            extract_openvpn_config_name(line),
            Some("office".to_string())
        );
    }

    #[test]
    fn test_extract_openvpn_config_name_conf_extension() {
        let line = "/usr/sbin/openvpn --config /etc/openvpn/home.conf";
        assert_eq!(extract_openvpn_config_name(line), Some("home".to_string()));
    }

    #[test]
    fn test_extract_openvpn_config_name_no_config_file() {
        let line = "openvpn --daemon --verb 3";
        assert_eq!(extract_openvpn_config_name(line), None);
    }

    #[test]
    fn test_extract_openvpn_config_name_empty_line() {
        assert_eq!(extract_openvpn_config_name(""), None);
    }

    // -- build_client_config ------------------------------------------------

    #[test]
    fn test_build_client_config_includes_all_fields() {
        let server = WireGuardServerInfo {
            public_key: "SERVERPUBKEY=".to_string(),
            listen_port: 51820,
            endpoint_host: Some("vpn.example.com".to_string()),
            endpoint_source: "manual".to_string(),
            endpoint_warning: None,
            running: true,
        };
        let config = build_client_config("PEERPRIVKEY=", "10.8.0.2", &server, "PSK=");

        assert!(config.contains("PrivateKey = PEERPRIVKEY="));
        assert!(config.contains("Address = 10.8.0.2/32"));
        assert!(config.contains("PublicKey = SERVERPUBKEY="));
        assert!(config.contains("PresharedKey = PSK="));
        assert!(config.contains("Endpoint = vpn.example.com:51820"));
        assert!(config.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
        assert!(config.contains("PersistentKeepalive = 25"));
    }

    #[test]
    fn test_build_client_config_falls_back_when_no_endpoint_detected() {
        let server = WireGuardServerInfo {
            public_key: "SERVERPUBKEY=".to_string(),
            listen_port: 51820,
            endpoint_host: None,
            endpoint_source: "unset".to_string(),
            endpoint_warning: Some("unset".to_string()),
            running: true,
        };
        let config = build_client_config("PEERPRIVKEY=", "10.8.0.5", &server, "PSK=");
        assert!(config.contains("Endpoint = YOUR-SERVER-IP:51820"));
    }

    // -- WireGuard client: parsing / rendering --------------------------------

    const CLIENT_KEY_A: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const CLIENT_KEY_B: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
    const CLIENT_KEY_C: &str = "AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=";

    fn sample_client_config() -> String {
        format!(
            "[Interface]\n\
             PrivateKey = {CLIENT_KEY_A}\n\
             Address = 10.2.0.2/32, fd00::2/128\n\
             DNS = 10.2.0.1\n\
             MTU = 1420\n\n\
             [Peer]\n\
             PublicKey = {CLIENT_KEY_B}\n\
             PresharedKey = {CLIENT_KEY_C}\n\
             AllowedIPs = 0.0.0.0/0, ::/0\n\
             Endpoint = vpn.example.com:51820\n\
             PersistentKeepalive = 25\n"
        )
    }

    #[test]
    fn test_parse_wireguard_client_config_reads_every_supported_field() {
        let parsed = parse_wireguard_client_config(&sample_client_config()).expect("valid config");
        assert_eq!(parsed.private_key, CLIENT_KEY_A);
        assert_eq!(parsed.addresses, vec!["10.2.0.2/32", "fd00::2/128"]);
        assert_eq!(parsed.dns, vec!["10.2.0.1"]);
        assert_eq!(parsed.mtu, Some(1420));
        assert_eq!(parsed.listen_port, None);
        assert_eq!(parsed.peers.len(), 1);
        assert_eq!(parsed.peers[0].public_key, CLIENT_KEY_B);
        assert_eq!(parsed.peers[0].preshared_key.as_deref(), Some(CLIENT_KEY_C));
        assert_eq!(parsed.peers[0].allowed_ips, vec!["0.0.0.0/0", "::/0"]);
        assert_eq!(
            parsed.peers[0].endpoint.as_deref(),
            Some("vpn.example.com:51820")
        );
        assert_eq!(parsed.peers[0].persistent_keepalive, Some(25));
        assert_eq!(parsed.endpoint().as_deref(), Some("vpn.example.com:51820"));
        assert_eq!(parsed.allowed_ips(), vec!["0.0.0.0/0", "::/0"]);
    }

    #[test]
    fn test_parse_wireguard_client_config_accepts_multiple_peers_and_merges_routes() {
        let content = format!(
            "[Interface]\n\
             PrivateKey = {CLIENT_KEY_A}\n\
             Address = 10.9.0.2/32\n\n\
             [Peer]\n\
             PublicKey = {CLIENT_KEY_B}\n\
             AllowedIPs = 192.168.10.0/24\n\n\
             [Peer]\n\
             PublicKey = {CLIENT_KEY_C}\n\
             AllowedIPs = 192.168.10.0/24, 192.168.20.0/24\n\
             Endpoint = [2001:db8::1]:51820\n"
        );
        let parsed = parse_wireguard_client_config(&content).expect("valid config");
        assert_eq!(parsed.peers.len(), 2);
        assert_eq!(
            parsed.allowed_ips(),
            vec!["192.168.10.0/24", "192.168.20.0/24"]
        );
        assert_eq!(
            parsed.peers[1].endpoint.as_deref(),
            Some("[2001:db8::1]:51820")
        );
    }

    #[test]
    fn test_parse_wireguard_client_config_refuses_root_command_hooks() {
        for directive in ["PreUp", "PostUp", "PreDown", "PostDown"] {
            let content = format!(
                "[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2/32\n\
                 {directive} = /tmp/evil.sh\n\n[Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n"
            );
            let error = parse_wireguard_client_config(&content)
                .expect_err("a root command hook must be refused");
            assert!(
                error.contains(directive) && error.contains("root"),
                "wrong refusal for {directive}: {error}"
            );
        }
    }

    #[test]
    fn test_parse_wireguard_client_config_refuses_unknown_directives() {
        let content = format!(
            "[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2/32\n\
             SaveConfig = true\n\n[Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n"
        );
        let error = parse_wireguard_client_config(&content).expect_err("SaveConfig is refused");
        assert!(error.contains("SaveConfig"), "{error}");

        let content = format!(
            "[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2/32\n\
             Mystery = yes\n\n[Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n"
        );
        let error = parse_wireguard_client_config(&content).expect_err("unknown key is refused");
        assert!(error.contains("Mystery"), "{error}");
    }

    #[test]
    fn test_parse_wireguard_client_config_refuses_invalid_values() {
        let bad_key = format!(
            "[Interface]\nPrivateKey = not-a-key\nAddress = 10.2.0.2/32\n\n\
             [Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n"
        );
        assert!(parse_wireguard_client_config(&bad_key).is_err());

        let bad_cidr = format!(
            "[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2\n\n\
             [Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n"
        );
        assert!(parse_wireguard_client_config(&bad_cidr).is_err());

        let bad_dns = format!(
            "[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2/32\n\
             DNS = example.com\n\n[Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n"
        );
        assert!(parse_wireguard_client_config(&bad_dns).is_err());

        let bad_endpoint = format!(
            "[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2/32\n\n\
             [Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\nEndpoint = vpn.example.com\n"
        );
        assert!(parse_wireguard_client_config(&bad_endpoint).is_err());
    }

    #[test]
    fn test_parse_wireguard_client_config_requires_interface_and_peer() {
        let no_peer = format!("[Interface]\nPrivateKey = {CLIENT_KEY_A}\nAddress = 10.2.0.2/32\n");
        assert!(parse_wireguard_client_config(&no_peer)
            .unwrap_err()
            .contains("[Peer]"));

        let no_interface = format!("[Peer]\nPublicKey = {CLIENT_KEY_B}\nAllowedIPs = 0.0.0.0/0\n");
        assert!(parse_wireguard_client_config(&no_interface).is_err());
    }

    #[test]
    fn test_render_wireguard_client_config_round_trips_and_has_no_hook_by_default() {
        let parsed = parse_wireguard_client_config(&sample_client_config()).expect("valid config");
        let rendered = render_wireguard_client_config(&parsed, false);
        assert!(rendered.contains("PrivateKey = "));
        assert!(rendered.contains("Address = 10.2.0.2/32, fd00::2/128"));
        assert!(rendered.contains("DNS = 10.2.0.1"));
        assert!(rendered.contains("MTU = 1420"));
        assert!(rendered.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
        assert!(rendered.contains("Endpoint = vpn.example.com:51820"));
        assert!(!rendered.contains("PostUp"));
        assert!(!rendered.contains("PreDown"));

        // The canonical form is itself parseable (no operator-owned hook).
        let reparsed = parse_wireguard_client_config(&rendered).expect("round trip");
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn test_render_wireguard_client_config_kill_switch_is_fixed() {
        let parsed = parse_wireguard_client_config(&sample_client_config()).expect("valid config");
        let rendered = render_wireguard_client_config(&parsed, true);
        assert!(rendered.contains("PostUp = iptables -I OUTPUT"));
        assert!(rendered.contains("PreDown = iptables -D OUTPUT"));
        assert!(rendered.contains("ip6tables"));
    }

    #[test]
    fn test_wireguard_client_interface_is_deterministic_and_bounded() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let iface = wireguard_client_interface(id);
        assert_eq!(iface, "hvc-123e4567e8");
        assert!(iface.len() <= 15);
        assert!(iface.starts_with(WG_CLIENT_PREFIX));
        assert_ne!(iface, WG_INTERFACE);
        assert_eq!(iface, wireguard_client_interface(id));
    }

    #[test]
    fn test_wireguard_client_commands_are_argv_only() {
        let commands = wireguard_client_commands("hvc-abc");
        assert_eq!(commands.up, vec!["wg-quick", "up", "hvc-abc"]);
        assert_eq!(commands.down, vec!["wg-quick", "down", "hvc-abc"]);
        assert_eq!(commands.show_dump, vec!["wg", "show", "hvc-abc", "dump"]);
    }

    #[test]
    fn test_wireguard_client_import_rollback_plan_orders_down_before_removal() {
        assert_eq!(
            wireguard_client_import_rollback_plan(),
            [
                WireGuardClientRollbackStep::Down,
                WireGuardClientRollbackStep::RemoveConfig
            ]
        );
    }

    #[test]
    fn test_wireguard_client_config_path_stays_in_etc_wireguard() {
        let path = wireguard_client_config_path("hvc-abc");
        assert_eq!(path.to_string_lossy(), "/etc/wireguard/hvc-abc.conf");
    }

    #[test]
    fn test_stored_client_interface_is_validated_before_use() {
        assert!(validate_client_interface("hvc-1234abcd").is_ok());
        for invalid in ["wg0", "hvc-../etc", "hvc-a/b", "hvc-with-a-very-long-name"] {
            assert!(
                validate_client_interface(invalid).is_err(),
                "'{invalid}' must be refused"
            );
        }
    }

    // -- saved_config_names -----------------------------------------------

    #[test]
    fn test_saved_config_names_lists_matching_extension_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("office.ovpn"), "").unwrap();
        std::fs::write(dir.path().join("home.ovpn"), "").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "").unwrap();

        let mut names = saved_config_names(dir.path().to_str().unwrap(), "ovpn", &[]);
        names.sort();
        assert_eq!(names, vec!["home".to_string(), "office".to_string()]);
    }

    #[test]
    fn test_saved_config_names_excludes_already_active_connections() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("office.ovpn"), "").unwrap();

        let active = vec![VpnConnection {
            id: "office".to_string(),
            name: "office".to_string(),
            vpn_type: VpnType::OpenVPN,
            status: VpnStatus::Connected,
            interface: None,
            local_ip: None,
            remote_ip: None,
            public_key: None,
            endpoint: None,
            allowed_ips: vec![],
            created_at: None,
        }];
        let names = saved_config_names(dir.path().to_str().unwrap(), "ovpn", &active);
        assert!(names.is_empty());
    }

    #[test]
    fn test_saved_config_names_missing_directory_returns_empty() {
        let names = saved_config_names("/nonexistent/dir/for/sure", "ovpn", &[]);
        assert!(names.is_empty());
    }

    // -- validate_openvpn_config_content ------------------------------------

    #[test]
    fn test_validate_openvpn_content_accepts_benign_profile() {
        let content = "\
client\ndev tun\nproto udp\nremote vpn.example.com 1194\n\
ca ca.pem\ncert client.pem\nkey client.key\nremote-cert-tls server\n\
cipher AES-256-GCM\nauth-user-pass\nverb 3\n";
        assert!(validate_openvpn_config_content(content).is_ok());
    }

    #[test]
    fn test_validate_openvpn_content_rejects_script_exec_directives() {
        for directive in [
            "up",
            "down",
            "down-pre",
            "route-up",
            "ipchange",
            "learn-address",
            "client-connect",
            "client-disconnect",
            "auth-user-pass-verify",
            "tls-verify",
            "askpass",
        ] {
            let content = format!("client\ndev tun\n{directive} /tmp/evil.sh\n");
            let err = validate_openvpn_config_content(&content)
                .expect_err("exec directive must be refused");
            assert!(
                err.contains(&format!("'{}'", directive)),
                "wrong message for {directive}: {err}"
            );
        }
    }

    #[test]
    fn test_validate_openvpn_content_accepts_exec_word_as_value_not_directive() {
        // A remote hostname ending in "up" or a comment must not trip the
        // deny list — only whole-line directives count.
        let content = "client\n# route-up example comment\nremote backup-up.example.com 1194\n";
        assert!(validate_openvpn_config_content(content).is_ok());
    }

    #[test]
    fn test_validate_openvpn_content_rejects_leading_whitespace_directive() {
        // Indented exec hooks still execute — the parser must not care where
        // the directive starts on the line.
        let content = "client\n  down /tmp/evil.sh\n";
        assert!(validate_openvpn_config_content(content).is_err());
    }

    #[test]
    fn test_validate_openvpn_content_rejects_script_security_above_one() {
        assert!(validate_openvpn_config_content("script-security 2\n").is_err());
        assert!(validate_openvpn_config_content("script-security 5\n").is_err());
    }

    #[test]
    fn test_validate_openvpn_content_accepts_script_security_one() {
        assert!(validate_openvpn_config_content("script-security 1\n").is_ok());
        // Level-less script-security defaults to 1 in OpenVPN.
        assert!(validate_openvpn_config_content("script-security\n").is_ok());
    }

    #[test]
    fn test_validate_openvpn_content_is_case_insensitive() {
        assert!(validate_openvpn_config_content("UP /tmp/evil.sh\n").is_err());
        assert!(validate_openvpn_config_content("Script-Security 3\n").is_err());
    }

    // -- openvpn_connected_from_log ------------------------------------------

    #[test]
    fn test_openvpn_connected_marker_found() {
        assert!(openvpn_connected_from_log(
            "2026-10-21 10:58:11UDPv4 link remote ok\n2026-10-21 10:58:12 Initialization Sequence Completed\n"
        ));
    }

    #[test]
    fn test_openvpn_connected_marker_absent() {
        assert!(!openvpn_connected_from_log(
            "2026-10-21 read UDPv4 [ECONNREFUSED]: Connection refused\n"
        ));
        assert!(!openvpn_connected_from_log(""));
    }

    // -- openvpn_fatal_from_log ----------------------------------------------

    #[test]
    fn test_openvpn_fatal_prefers_exiting_lines() {
        let log = "Options error: early problem\n2026-10-21 10:59:52 Exiting due to fatal error\n";
        assert_eq!(
            openvpn_fatal_from_log(log),
            Some("2026-10-21 10:59:52 Exiting due to fatal error".to_string())
        );
    }

    #[test]
    fn test_openvpn_fatal_falls_back_to_transport_error() {
        let log =
            "2026-10-21 10:59:08 read UDPv4 [ECONNREFUSED]: Connection refused (fd=3,code=111)\n";
        assert_eq!(openvpn_fatal_from_log(log), Some(log.trim().to_string()));
    }

    #[test]
    fn test_openvpn_fatal_none_on_happy_log() {
        assert_eq!(
            openvpn_fatal_from_log("Initialization Sequence Completed\n"),
            None
        );
        assert_eq!(openvpn_fatal_from_log(""), None);
    }

    // -- openvpn_log_error_summary -------------------------------------------

    #[test]
    fn test_openvpn_log_error_summary_strips_timestamp() {
        assert_eq!(
            openvpn_log_error_summary("2026-10-21 10:59:52 Exiting due to fatal error"),
            "Exiting due to fatal error"
        );
    }

    #[test]
    fn test_openvpn_log_error_summary_options_error_has_no_timestamp() {
        // Parse-time errors are echoed without any timestamp prefix.
        assert_eq!(
            openvpn_log_error_summary(
                "Options error: --cert fails with 'cli.pem': No such file or directory (errno=2)"
            ),
            "Options error: --cert fails with 'cli.pem': No such file or directory (errno=2)"
        );
    }

    #[test]
    fn test_openvpn_log_error_summary_plain_message() {
        assert_eq!(openvpn_log_error_summary("bare message"), "bare message");
        assert_eq!(openvpn_log_error_summary(""), "");
    }

    // -- subprocess deadlines (ROADMAP X.2) -----------------------------

    /// The defect this guards against is invisible to a behavioural test: a
    /// bare command call still answers correctly on an idle box and only pins
    /// the caller when `wg`/`wg-quick`/`openvpn`/`pgrep` stops answering — and
    /// the VPN screen polls the server status and the peer list. So the
    /// invariant is asserted at the source.
    ///
    /// `concat!` keeps each pattern out of this file's own source, so the
    /// assertion cannot count itself.
    #[test]
    fn test_no_subprocess_of_this_module_runs_without_a_deadline() {
        let src = include_str!("vpn.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for unbounded in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!(".", "status()"),
            concat!("wait_with_", "output()"),
        ] {
            assert!(
                !production.contains(unbounded),
                "`{unbounded}` waits without a deadline — use `run_command`/`run_timed_detached` (ROADMAP X.2)"
            );
        }
        // `openvpn --daemon` specifically must go through the detached helper:
        // its forked child keeps any inherited pipe open, so a piping helper
        // would make the deadline unfireable and park the start call forever.
        assert!(
            production.contains("run_timed_detached("),
            "the OpenVPN daemon start must use `run_timed_detached` (ROADMAP X.2)"
        );
    }

    #[test]
    fn test_run_command_kills_a_hung_tool_and_names_the_deadline() {
        let started = std::time::Instant::now();
        let error = run_command(Command::new("sleep").arg("30"), Duration::from_millis(120))
            .expect_err("a hung tool must not succeed");
        assert!(error.contains("did not finish within"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must be enforced, not waited out"
        );
    }

    #[test]
    fn test_run_command_keeps_a_non_zero_exit_as_data() {
        // `wg show` on a stopped interface exits non-zero and the caller reads
        // that as "not running"; a non-zero exit must arrive as output, never
        // as an error.
        let out = run_command(&mut Command::new("false"), Duration::from_secs(5))
            .expect("`false` still runs to completion");
        assert_eq!(out.status.code(), Some(1));
    }

    #[test]
    fn test_run_command_reports_a_missing_tool() {
        let error = run_command(
            &mut Command::new("homevault-no-such-binary"),
            Duration::from_secs(1),
        )
        .expect_err("a missing tool is an error");
        assert!(error.contains("Failed to run"), "{error}");
    }

    #[test]
    fn test_vpn_deadlines_are_ceilings_and_ordered() {
        // A probe is the shortest; a root mutation (or an `openvpn --daemon`
        // parent) may wait on the sudo/PAM stack or on the kernel.
        assert!(VPN_PROBE_TIMEOUT >= Duration::from_secs(1));
        assert!(VPN_WRITE_TIMEOUT >= VPN_PROBE_TIMEOUT);
        // Still finite: a wedged `wg-quick up` must free the caller.
        assert!(VPN_WRITE_TIMEOUT <= Duration::from_secs(120));
        // The daemon start must sit above the parent's own grace, or the
        // verification loop could outlive the deadline that killed it.
        assert!(VPN_WRITE_TIMEOUT >= OPENVPN_START_GRACE);
    }

    #[test]
    fn test_validate_endpoint_host_accepts_ip_and_dns_names() {
        assert_eq!(validate_endpoint_host("").unwrap(), "");
        assert_eq!(
            validate_endpoint_host("203.0.113.7").unwrap(),
            "203.0.113.7"
        );
        assert_eq!(
            validate_endpoint_host("2001:db8::1").unwrap(),
            "2001:db8::1"
        );
        assert_eq!(
            validate_endpoint_host("Home.Example.com").unwrap(),
            "home.example.com"
        );
        // A scheme or a port would be spliced into `Endpoint = host:port` and
        // produce an unreachable config.
        assert!(validate_endpoint_host("https://home.example.com").is_err());
        assert!(validate_endpoint_host("home.example.com:51820").is_err());
        assert!(validate_endpoint_host("not a host").is_err());
    }

    #[test]
    fn test_endpoint_warning_flags_unset_and_private_addresses() {
        assert!(endpoint_warning(None).is_some());
        assert!(endpoint_warning(Some("")).is_some());
        assert!(endpoint_warning(Some("192.168.1.10")).is_some());
        assert!(endpoint_warning(Some("10.0.0.5")).is_some());
        assert!(endpoint_warning(Some("100.64.0.1")).is_some()); // CGNAT
        assert!(endpoint_warning(Some("127.0.0.1")).is_some());
        assert!(endpoint_warning(Some("fd00::1")).is_some());
        // A public IP or a DNS name is not obviously wrong.
        assert!(endpoint_warning(Some("203.0.113.7")).is_none());
        assert!(endpoint_warning(Some("home.example.com")).is_none());
    }
}
