use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

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
    Command::new("sudo")
        .args(["test", "-e", &wg_server_config_path()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Reads the WireGuard server config via `sudo cat`. The file is
/// deliberately `0600` (it holds the server's own private key), so unlike
/// the other root-owned configs this codebase reads directly (`/etc/fstab`,
/// `smb.conf`, ...) — which stay default-readable — even the `homevault`
/// service account can't read this one back with a plain
/// `std::fs::read_to_string` once it's been written.
fn read_wg_config() -> Result<String, String> {
    let output = Command::new("sudo")
        .args(["cat", &wg_server_config_path()])
        .output()
        .map_err(|e| e.to_string())?;
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
    let output = Command::new("sudo")
        .args(["chmod", mode, &wg_server_config_path()])
        .output()
        .map_err(|e| e.to_string())?;
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
    Command::new("wg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detects the interface carrying this host's default route, so the
/// server's NAT rule (below) knows which interface to masquerade behind —
/// without it, a peer with a full-tunnel `AllowedIPs = 0.0.0.0/0` could
/// reach the server itself but never get real internet access through it.
fn detect_egress_interface() -> Option<String> {
    let output = Command::new("ip")
        .args(["route", "get", "1.1.1.1"])
        .output()
        .ok()?;
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
    Command::new("sudo")
        .args(["wg", "show", WG_INTERFACE])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Brings up `wg0` via `wg-quick`, which also runs the config's PostUp
/// NAT/forwarding rules. Treats "already exists" (the interface was already
/// up from an earlier call) as success rather than an error.
fn start_wireguard_server() -> Result<(), String> {
    let output = Command::new("sudo")
        .args(["wg-quick", "up", WG_INTERFACE])
        .output()
        .map_err(|e| e.to_string())?;

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
    let _ = Command::new("sudo")
        .args(["systemctl", "enable", &format!("wg-quick@{}", WG_INTERFACE)])
        .output();
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
        );
    } else if !is_wireguard_server_running() {
        start_wireguard_server()?;
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
    pub running: bool,
}

pub async fn get_wireguard_server_status() -> Result<WireGuardServerInfo, String> {
    let public_key = read_server_public_key()?;
    let running = is_wireguard_server_running();
    let endpoint_host = crate::network::discover_public_ip()
        .await
        .map(|ip| ip.to_string());

    Ok(WireGuardServerInfo {
        public_key,
        listen_port: WG_SERVER_PORT,
        endpoint_host,
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
    let output = Command::new("wg")
        .arg("pubkey")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(private_key.as_bytes())?;
            }
            child.wait_with_output()
        })
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err("Failed to derive public key".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Generate a WireGuard key pair
pub fn generate_wireguard_keypair() -> Result<(String, String), String> {
    let private_output = Command::new("wg")
        .arg("genkey")
        .output()
        .map_err(|e| e.to_string())?;

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
    let output = Command::new("wg")
        .arg("genpsk")
        .output()
        .map_err(|e| e.to_string())?;

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

    let set_output = Command::new("sudo")
        .args([
            "wg",
            "set",
            WG_INTERFACE,
            "peer",
            &peer_public_key,
            "preshared-key",
            "/dev/stdin",
            "allowed-ips",
            &format!("{}/32", peer_ip),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(preshared_key.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
            child.wait_with_output()
        })
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
    let _ = Command::new("sudo")
        .args(["wg", "set", WG_INTERFACE, "peer", public_key, "remove"])
        .output();

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
    let output = Command::new("sudo")
        .args(["wg", "show", WG_INTERFACE, "dump"])
        .output();

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

/// Check if OpenVPN is available
pub fn openvpn_available() -> bool {
    Command::new("openvpn")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// List all OpenVPN connections
pub fn list_openvpn_connections() -> Vec<VpnConnection> {
    let mut connections = Vec::new();

    // Check running OpenVPN processes
    let output = Command::new("pgrep").args(["-a", "openvpn"]).output();

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

/// Start an OpenVPN connection
pub fn start_openvpn_connection(config: &OpenVPNConfig) -> Result<(), String> {
    let config_path = &config.config_file;

    if !Path::new(config_path).exists() {
        return Err(format!("Config file not found: {}", config_path));
    }

    // `openvpn --daemon` must run as root to create the tunnel.
    // The homevault service runs unprivileged, so it delegates via sudo.
    let mut cmd = Command::new("sudo");
    cmd.arg("openvpn")
        .args(["--config", config_path, "--daemon"]);

    // Add auth credentials if provided
    if let Some((user, pass)) = &config.auth_user_pass {
        let auth_file = format!("/run/openvpn-auth-{}.txt", config.name);
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

    let output = cmd.output().map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to start OpenVPN: {}", stderr));
    }

    Ok(())
}

/// Stop an OpenVPN connection
pub fn stop_openvpn_connection(name: &str) -> Result<(), String> {
    // Find OpenVPN process with this config
    let output = Command::new("pgrep")
        .args(["-f", &format!("openvpn.*{}", name)])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid in pids.lines() {
            let pid = pid.trim();
            // Validate PID is numeric to prevent command injection
            if !pid.chars().all(|c| c.is_ascii_digit()) || pid.is_empty() {
                continue;
            }
            // The openvpn process runs as root, so the unprivileged
            // homevault service must delegate the kill to sudo.
            let _ = Command::new("sudo").args(["kill", pid]).output();
        }
    }

    Ok(())
}

/// Upload and save OpenVPN config
pub fn save_openvpn_config(name: &str, config_content: &str) -> Result<String, String> {
    let config_dir = "/etc/openvpn";
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
    let config_path = format!("/etc/openvpn/{}.ovpn", name);
    if Path::new(&config_path).exists() {
        std::fs::remove_file(&config_path)
            .map_err(|e| format!("Failed to delete config: {}", e))?;
    }

    Ok(())
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

    for name in saved_config_names("/etc/openvpn", "ovpn", &connections) {
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
#[derive(Debug, Serialize)]
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
            running: true,
        };
        let config = build_client_config("PEERPRIVKEY=", "10.8.0.5", &server, "PSK=");
        assert!(config.contains("Endpoint = YOUR-SERVER-IP:51820"));
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
}
