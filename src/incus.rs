//! Incus REST API client over its Unix socket (`/var/lib/incus/unix.socket`).
//!
//! All VM / system-container management goes through this module — no `virsh`,
//! no libvirt, no XML. Plain HTTP/1 over `tokio::net::UnixStream` via `hyper`,
//! plus one raw WebSocket path (`open_operation_ws`) for the interactive
//! console relay.
//!
//! The public surface is deliberately small and shaped for the "a normal
//! person runs one or two VMs at home" use case: a curated OS catalogue, sane
//! size presets, a seeded login account, and a shell console — see
//! `docs/guides/vm.md`.

use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tokio::net::UnixStream;

const SOCKET_PATH: &str = "/var/lib/incus/unix.socket";

/// Incus happily sends `"field": null` where a map/array is expected (e.g.
/// `state.disk` on a VM with no disk stats yet). Plain `#[serde(default)]` only
/// covers an *absent* field, not a present-but-null one — this covers both.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Incus instance status codes we care about (from the REST API).
pub const STATUS_RUNNING: u16 = 103;
pub const STATUS_STOPPED: u16 = 102;

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum IncusError {
    #[error("Incus is not installed or its service is not running")]
    NotAvailable,
    #[error("Machine '{0}' was not found")]
    InstanceNotFound(String),
    /// A background Incus operation finished with an error (image download
    /// failed, not enough disk, name already taken, …). The string is the
    /// human-readable reason straight from Incus.
    #[error("{0}")]
    Operation(String),
    #[error("I/O error talking to Incus: {0}")]
    Io(#[from] std::io::Error),
    #[error("Incus returned HTTP {0}: {1}")]
    Http(u16, String),
    #[error("could not parse the Incus response: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("connection to Incus failed: {0}")]
    Transport(String),
}

impl IncusError {
    /// Maps to an HTTP status for callers that want one. Kept for completeness;
    /// the VM handlers currently answer 200/`{success:false}` like Docker.
    pub fn status_code(&self) -> u16 {
        match self {
            IncusError::NotAvailable | IncusError::Io(_) | IncusError::Transport(_) => 503,
            IncusError::InstanceNotFound(_) => 404,
            IncusError::Http(code, _) => *code,
            IncusError::Operation(_) | IncusError::Serde(_) => 500,
        }
    }
}

// ── Wire types ──────────────────────────────────────────────────────────────

/// One instance from `GET /1.0/instances?recursion=2` — the recursion pulls in
/// `state` and `snapshots` in the same round-trip, so the list view needs no
/// follow-up calls.
#[derive(Debug, Clone, Deserialize)]
pub struct IncusInstance {
    pub name: String,
    #[serde(rename = "type")]
    pub instance_type: String,
    pub status: String,
    pub status_code: u16,
    #[serde(default)]
    pub architecture: String,
    #[serde(default, deserialize_with = "null_default")]
    pub config: HashMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "null_default")]
    pub devices: HashMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "null_default")]
    pub expanded_devices: HashMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "null_default")]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    /// Present only with `recursion=2`.
    #[serde(default)]
    pub state: Option<IncusInstanceState>,
    /// Present only with `recursion=2`.
    #[serde(default)]
    pub snapshots: Option<Vec<IncusSnapshot>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncusInstanceState {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub status_code: u16,
    #[serde(default)]
    pub pid: Option<i64>,
    #[serde(default, deserialize_with = "null_default")]
    pub disk: HashMap<String, IncusDiskState>,
    #[serde(default)]
    pub memory: Option<IncusMemoryState>,
    #[serde(default)]
    pub network: Option<HashMap<String, IncusNetworkState>>,
    #[serde(default)]
    pub cpu: Option<IncusCpuState>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncusDiskState {
    #[serde(default)]
    pub usage: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncusMemoryState {
    #[serde(default)]
    pub usage: i64,
    #[serde(default)]
    pub usage_peak: i64,
    #[serde(default)]
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncusCpuState {
    #[serde(default)]
    pub usage: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncusNetworkState {
    #[serde(default)]
    pub addresses: Vec<IncusNetworkAddress>,
    #[serde(default)]
    pub hwaddr: String,
    #[serde(default)]
    pub state: String,
    #[serde(rename = "type", default)]
    pub net_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncusNetworkAddress {
    pub address: String,
    pub family: String,
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncusSnapshot {
    pub name: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub stateful: bool,
}

// ── Create request ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CreateInstanceBody {
    pub name: String,
    #[serde(rename = "type")]
    pub instance_type: String,
    pub source: InstanceSource,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub devices: HashMap<String, serde_json::Value>,
    pub profiles: Vec<String>,
    pub start: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InstanceSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Serialize)]
struct StateActionBody {
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<i64>,
}

#[derive(Serialize)]
struct SnapshotBody {
    name: String,
    stateful: bool,
}

#[derive(Serialize)]
struct RestoreBody {
    restore: String,
}

#[derive(Serialize)]
struct ConfigPatchBody {
    config: HashMap<String, String>,
}

#[derive(Serialize)]
struct ExecBody {
    command: Vec<String>,
    #[serde(rename = "wait-for-websocket")]
    wait_for_websocket: bool,
    interactive: bool,
    environment: HashMap<String, String>,
    width: u32,
    height: u32,
}

/// The secrets Incus hands back for an interactive `exec` — one bidirectional
/// data channel (`fd_0`) and one control channel (resize / signals).
#[derive(Debug, Clone)]
pub struct ExecSession {
    pub operation_id: String,
    pub fd_0_secret: String,
    pub control_secret: String,
}

// ── OS catalogue ────────────────────────────────────────────────────────────

/// One entry a normal person can pick from when creating a machine. `alias` is
/// resolved against the public LXC image server (`images:` remote).
#[derive(Debug, Clone, Serialize)]
pub struct OsOption {
    pub id: &'static str,
    pub label: &'static str,
    /// `"vm"` = KVM guest, `"container"` = system container. Some entries only
    /// make sense one way (a desktop image wants a VM); most support both and
    /// the UI lets the person choose.
    pub kinds: &'static [&'static str],
    pub alias: &'static str,
    /// Short "what is this / who is it for" line, already in plain language.
    pub blurb: &'static str,
    pub recommended: bool,
    /// No ready-made image exists (Windows): the wizard must send the person
    /// to the ISO install instead of offering a one-click create.
    pub iso_only: bool,
}

/// The curated list. All of these ship cloud-init, so the seeded login account
/// works on first boot.
pub fn os_catalogue() -> &'static [OsOption] {
    &[
        OsOption {
            id: "ubuntu-24.04",
            label: "Ubuntu 24.04 LTS",
            kinds: &["vm", "container"],
            alias: "ubuntu/24.04/cloud",
            blurb: "The most common choice. Great for web apps, tools and learning Linux.",
            recommended: true,
            iso_only: false,
        },
        OsOption {
            id: "ubuntu-22.04",
            label: "Ubuntu 22.04 LTS",
            kinds: &["vm", "container"],
            alias: "ubuntu/22.04/cloud",
            blurb: "Older long-term-support Ubuntu, for software that needs it.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "debian-12",
            label: "Debian 12",
            kinds: &["vm", "container"],
            alias: "debian/12/cloud",
            blurb: "Rock-solid and lightweight. A good default for servers.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "fedora-43",
            label: "Fedora 43",
            kinds: &["vm", "container"],
            alias: "fedora/43/cloud",
            blurb: "Cutting-edge packages, backed by Red Hat.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "rocky-9",
            label: "Rocky Linux 9",
            kinds: &["vm", "container"],
            alias: "rockylinux/9/cloud",
            blurb: "A free stand-in for Red Hat Enterprise Linux.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "alpine-3.21",
            label: "Alpine 3.21",
            kinds: &["container"],
            alias: "alpine/3.21/cloud",
            blurb: "Tiny. Boots in a blink — handy for a single small service.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "ubuntu-24.04-minimal",
            label: "Ubuntu 24.04 Minimal",
            kinds: &["vm", "container"],
            alias: "ubuntu/24.04/cloud",
            blurb: "Same Ubuntu base with a small disk and 1 GB of RAM — for one lightweight service.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "debian-12-minimal",
            label: "Debian 12 Minimal",
            kinds: &["vm", "container"],
            alias: "debian/12/cloud",
            blurb: "The smallest usual Debian footprint, for DNS, VPN or file helpers.",
            recommended: false,
            iso_only: false,
        },
        OsOption {
            id: "windows-11",
            label: "Windows 11",
            kinds: &["vm"],
            alias: "windows/11",
            blurb: "For Windows-only software. Install from your own ISO — a TPM and Secure Boot are wired up automatically.",
            recommended: false,
            iso_only: true,
        },
        OsOption {
            id: "windows-10",
            label: "Windows 10",
            kinds: &["vm"],
            alias: "windows/10",
            blurb: "For older Windows-only software. Install from your own ISO.",
            recommended: false,
            iso_only: true,
        },
    ]
}

/// Resolve a catalogue id to its image alias. Unknown ids fall back to the
/// recommended entry so the create call can still succeed with a sane default.
pub fn resolve_os_alias(id: &str) -> &'static str {
    os_catalogue()
        .iter()
        .find(|o| o.id == id)
        .or_else(|| os_catalogue().iter().find(|o| o.recommended))
        .map(|o| o.alias)
        .unwrap_or("ubuntu/24.04/cloud")
}

/// QEMU audio device attached when the person ticks "sound" in the wizard
/// (VMs only — containers share the host audio stack differently). `intel-hda`
/// with an `hda-micro` codec is the combination guest drivers expect without
/// extra setup.
pub fn audio_device() -> serde_json::Value {
    serde_json::json!({
        "type": "qemu",
        "driver": "intel-hda",
        "codec": "hda-micro",
    })
}

// ── Service ─────────────────────────────────────────────────────────────────

pub struct IncusService;

impl IncusService {
    /// Is the Incus daemon reachable?
    pub async fn is_available() -> bool {
        Self::request_json("GET", "/1.0", None).await.is_ok()
    }

    // ---- instances ----

    pub async fn list_instances() -> Result<Vec<IncusInstance>, IncusError> {
        let v = Self::request_json("GET", "/1.0/instances?recursion=2", None).await?;
        Self::metadata(v)
    }

    pub async fn get_instance(name: &str) -> Result<IncusInstance, IncusError> {
        let path = format!("/1.0/instances/{}?recursion=1", enc(name));
        match Self::request_json("GET", &path, None).await {
            Ok(v) => Self::metadata(v),
            Err(IncusError::Http(404, _)) => Err(IncusError::InstanceNotFound(name.to_string())),
            Err(e) => Err(e),
        }
    }

    pub async fn get_instance_state(name: &str) -> Result<IncusInstanceState, IncusError> {
        let path = format!("/1.0/instances/{}/state", enc(name));
        let v = Self::request_json("GET", &path, None).await?;
        Self::metadata(v)
    }

    /// POST a new instance. Returns the operation id (`/1.0/operations/<uuid>`)
    /// — the caller decides whether to wait.
    pub async fn create_instance(body: &CreateInstanceBody) -> Result<String, IncusError> {
        let payload = serde_json::to_vec(body)?;
        let v = Self::request_json("POST", "/1.0/instances", Some(&payload)).await?;
        Self::operation_id(&v)
    }

    pub async fn set_state(
        name: &str,
        action: &str,
        force: bool,
        timeout: Option<i64>,
    ) -> Result<String, IncusError> {
        let body = StateActionBody {
            action: action.to_string(),
            force: if force { Some(true) } else { None },
            timeout,
        };
        let payload = serde_json::to_vec(&body)?;
        let path = format!("/1.0/instances/{}/state", enc(name));
        let v = Self::request_json("PUT", &path, Some(&payload)).await?;
        Self::operation_id(&v)
    }

    pub async fn delete_instance(name: &str) -> Result<String, IncusError> {
        let path = format!("/1.0/instances/{}", enc(name));
        let v = Self::request_json("DELETE", &path, None).await?;
        Self::operation_id(&v)
    }

    pub async fn update_config(
        name: &str,
        config: HashMap<String, String>,
    ) -> Result<(), IncusError> {
        let payload = serde_json::to_vec(&ConfigPatchBody { config })?;
        let path = format!("/1.0/instances/{}", enc(name));
        Self::request_json("PATCH", &path, Some(&payload)).await?;
        Ok(())
    }

    /// Remove one device from an instance. Incus `PATCH` does **not** drop a
    /// device given `null`, so this is a read-modify-`PUT` of the whole config.
    pub async fn remove_device(name: &str, device: &str) -> Result<(), IncusError> {
        let inst = Self::get_instance(name).await?;
        if !inst.devices.contains_key(device) {
            return Ok(());
        }
        let mut devices = inst.devices.clone();
        devices.remove(device);
        let body = serde_json::json!({
            "architecture": inst.architecture,
            "config": inst.config,
            "devices": devices,
            "profiles": if inst.profiles.is_empty() { vec!["default".to_string()] } else { inst.profiles.clone() },
        });
        let payload = serde_json::to_vec(&body)?;
        let path = format!("/1.0/instances/{}", enc(name));
        Self::request_json("PUT", &path, Some(&payload)).await?;
        Ok(())
    }

    // ---- snapshots ----

    pub async fn list_snapshots(name: &str) -> Result<Vec<IncusSnapshot>, IncusError> {
        let path = format!("/1.0/instances/{}/snapshots?recursion=1", enc(name));
        let v = Self::request_json("GET", &path, None).await?;
        Self::metadata(v)
    }

    pub async fn create_snapshot(
        name: &str,
        snap: &str,
        stateful: bool,
    ) -> Result<String, IncusError> {
        let payload = serde_json::to_vec(&SnapshotBody {
            name: snap.to_string(),
            stateful,
        })?;
        let path = format!("/1.0/instances/{}/snapshots", enc(name));
        let v = Self::request_json("POST", &path, Some(&payload)).await?;
        Self::operation_id(&v)
    }

    pub async fn delete_snapshot(name: &str, snap: &str) -> Result<String, IncusError> {
        let path = format!("/1.0/instances/{}/snapshots/{}", enc(name), enc(snap));
        let v = Self::request_json("DELETE", &path, None).await?;
        Self::operation_id(&v)
    }

    /// Roll the instance back to a snapshot (`PUT` with `{"restore": ...}`).
    pub async fn restore_snapshot(name: &str, snap: &str) -> Result<String, IncusError> {
        let payload = serde_json::to_vec(&RestoreBody {
            restore: snap.to_string(),
        })?;
        let path = format!("/1.0/instances/{}", enc(name));
        let v = Self::request_json("PUT", &path, Some(&payload)).await?;
        Self::operation_id(&v)
    }

    // ---- operations ----

    /// Block until `op_id` finishes. `op_id` is what `create_instance` etc.
    /// return (`/1.0/operations/<uuid>`). A non-200 operation status becomes
    /// `IncusError::Operation(reason)`.
    pub async fn wait_for_operation(op_id: &str, timeout_secs: u64) -> Result<(), IncusError> {
        let path = format!(
            "{}/wait?timeout={}",
            op_id.trim_end_matches('/'),
            timeout_secs
        );
        let v = Self::request_json("GET", &path, None).await?;
        let meta = v
            .get("metadata")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let code = meta
            .get("status_code")
            .and_then(|c| c.as_u64())
            .unwrap_or(0);
        if code == 200 {
            return Ok(());
        }
        let reason = meta
            .get("err")
            .and_then(|e| e.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| meta.get("status").and_then(|s| s.as_str()))
            .unwrap_or("the operation did not complete")
            .to_string();
        Err(IncusError::Operation(reason))
    }

    // ---- interactive console ----

    /// Start an interactive `exec` (a login shell). Works for containers
    /// immediately; for VMs it needs the Incus guest agent to be up (it is, a
    /// short while after boot, on every image in our catalogue).
    pub async fn exec_shell(name: &str, cols: u32, rows: u32) -> Result<ExecSession, IncusError> {
        let mut environment = HashMap::new();
        environment.insert("TERM".to_string(), "xterm-256color".to_string());
        environment.insert("HOME".to_string(), "/root".to_string());
        let body = ExecBody {
            // `su -l root` gives a proper login shell with a fresh environment;
            // falls back through the shells most images actually ship.
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "exec $(command -v bash || command -v ash || command -v sh) -l".to_string(),
            ],
            wait_for_websocket: true,
            interactive: true,
            environment,
            width: cols.clamp(20, 500),
            height: rows.clamp(5, 300),
        };
        let payload = serde_json::to_vec(&body)?;
        let path = format!("/1.0/instances/{}/exec", enc(name));
        let v = Self::request_json("POST", &path, Some(&payload)).await?;

        let op_id = Self::operation_id(&v)?;
        let fds = v
            .get("metadata")
            .and_then(|m| m.get("metadata"))
            .and_then(|m| m.get("fds"))
            .ok_or_else(|| {
                IncusError::Operation("Incus did not return console channels".to_string())
            })?;
        let fd_0 = fds
            .get("0")
            .and_then(|s| s.as_str())
            .ok_or_else(|| IncusError::Operation("missing console data channel".to_string()))?
            .to_string();
        let control = fds
            .get("control")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(ExecSession {
            operation_id: op_id,
            fd_0_secret: fd_0,
            control_secret: control,
        })
    }

    /// Open a raw WebSocket to an operation channel over the Incus Unix socket.
    /// Used by the console relay for both the data and the control channel.
    pub async fn open_operation_ws(
        op_id: &str,
        secret: &str,
    ) -> Result<tokio_tungstenite::WebSocketStream<UnixStream>, IncusError> {
        let uuid = op_id.rsplit('/').next().unwrap_or(op_id);
        let uri = format!(
            "ws://localhost/1.0/operations/{}/websocket?secret={}",
            uuid, secret
        );
        let stream = UnixStream::connect(SOCKET_PATH)
            .await
            .map_err(IncusError::Io)?;
        let (ws, _resp) = tokio_tungstenite::client_async(uri, stream)
            .await
            .map_err(|e| IncusError::Transport(e.to_string()))?;
        Ok(ws)
    }

    // ── HTTP transport over the Unix socket ─────────────────────────────────

    async fn request_json(
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<serde_json::Value, IncusError> {
        // Never let a slow/stuck Incus hang a HomeVault request forever.
        // (`create_instance` etc. return an operation URL immediately; the
        // long wait is `wait_for_operation`, which has its own `?timeout=`.)
        match tokio::time::timeout(
            std::time::Duration::from_secs(45),
            Self::request_json_inner(method, path, body),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(IncusError::Transport(
                "Incus did not respond in time".to_string(),
            )),
        }
    }

    async fn request_json_inner(
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<serde_json::Value, IncusError> {
        let stream = match UnixStream::connect(SOCKET_PATH).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(IncusError::NotAvailable)
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                return Err(IncusError::NotAvailable)
            }
            // A non-root process that cannot open the socket — treat it as
            // "Incus not usable here" rather than a scary I/O error. HomeVault
            // runs as root in production, so this is really a dev-box case.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(IncusError::NotAvailable)
            }
            Err(e) => return Err(IncusError::Io(e)),
        };
        let io = TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|e| IncusError::Transport(e.to_string()))?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                log::debug!("Incus HTTP connection closed: {}", e);
            }
        });

        let mut builder = hyper::Request::builder()
            .method(method)
            .uri(format!("http://localhost{}", path))
            .header("Host", "localhost")
            .header("Accept", "application/json");
        let request = match body {
            Some(b) => {
                builder = builder.header("Content-Type", "application/json");
                builder
                    .body(http_body_util::Full::new(Bytes::from(b.to_vec())))
                    .map_err(|e| IncusError::Transport(e.to_string()))?
            }
            None => builder
                .body(http_body_util::Full::new(Bytes::new()))
                .map_err(|e| IncusError::Transport(e.to_string()))?,
        };

        let resp = sender
            .send_request(request)
            .await
            .map_err(|e| IncusError::Transport(e.to_string()))?;
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| IncusError::Transport(e.to_string()))?
            .to_bytes();

        let value: serde_json::Value = serde_json::from_slice(&bytes)?;

        // Incus signals errors both by HTTP status and by an `{"type":"error"}`
        // envelope — treat either as an error.
        if value.get("type").and_then(|t| t.as_str()) == Some("error") {
            let code = value
                .get("error_code")
                .and_then(|c| c.as_u64())
                .unwrap_or(status.as_u16() as u64) as u16;
            let msg = value
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown Incus error")
                .to_string();
            return Err(IncusError::Http(code, msg));
        }
        if !status.is_success() {
            let msg = value
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("request failed")
                .to_string();
            return Err(IncusError::Http(status.as_u16(), msg));
        }
        Ok(value)
    }

    fn metadata<T: for<'de> Deserialize<'de>>(v: serde_json::Value) -> Result<T, IncusError> {
        let meta = v
            .get("metadata")
            .cloned()
            .ok_or_else(|| IncusError::Operation("Incus response had no data".to_string()))?;
        serde_json::from_value(meta).map_err(IncusError::Serde)
    }

    fn operation_id(v: &serde_json::Value) -> Result<String, IncusError> {
        Ok(v.get("operation")
            .and_then(|o| o.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string())
    }
}

/// URL-encode a single path segment (instance / snapshot name).
fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

// ── cloud-init ──────────────────────────────────────────────────────────────

/// Build the `#cloud-config` user-data that seeds the login account on a fresh
/// machine. The password is stored **only** as a SHA-512 crypt hash — the
/// plaintext never lands in the instance config or any log.
pub fn build_cloud_init(username: &str, password_plain: &str) -> String {
    use sha_crypt::{PasswordHasher, ShaCrypt};

    let hash = ShaCrypt::SHA512
        .hash_password(password_plain.as_bytes())
        .map(|h| h.as_str().to_string())
        .unwrap_or_default();
    // YAML-safe: the username is already validated to [a-z0-9_-], the hash is
    // base64-ish crypt output with no special YAML characters.
    format!(
        "#cloud-config\n\
         users:\n\
         \x20\x20- name: {user}\n\
         \x20\x20\x20\x20groups: [sudo, wheel, adm]\n\
         \x20\x20\x20\x20sudo: \"ALL=(ALL) NOPASSWD:ALL\"\n\
         \x20\x20\x20\x20shell: /bin/bash\n\
         \x20\x20\x20\x20lock_passwd: false\n\
         \x20\x20\x20\x20passwd: \"{hash}\"\n\
         ssh_pwauth: true\n\
         disable_root: false\n\
         chpasswd:\n\
         \x20\x20expire: false\n",
        user = username,
        hash = hash,
    )
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha_crypt::PasswordVerifier;

    #[test]
    fn enc_escapes_path_segments() {
        assert_eq!(enc("my-vm"), "my-vm");
        assert_eq!(enc("my vm"), "my%20vm");
        assert_eq!(enc("a/b"), "a%2Fb");
        assert_eq!(enc("a..b"), "a..b");
    }

    #[test]
    fn metadata_extracts_list() {
        let v = json!({
            "type": "sync",
            "metadata": [
                {"name": "web", "type": "virtual-machine", "status": "Running", "status_code": 103},
                {"name": "db", "type": "container", "status": "Stopped", "status_code": 102}
            ]
        });
        let list: Vec<IncusInstance> = IncusService::metadata(v).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "web");
        assert_eq!(list[0].instance_type, "virtual-machine");
        assert_eq!(list[1].status_code, 102);
    }

    #[test]
    fn metadata_parses_recursion2_state_and_snapshots() {
        let v = json!({
            "type": "sync",
            "metadata": [{
                "name": "web",
                "type": "virtual-machine",
                "status": "Running",
                "status_code": 103,
                "config": {"limits.cpu": "2", "limits.memory": "2048MiB", "image.description": "Ubuntu 24.04"},
                "devices": {"root": {"type": "disk", "path": "/", "size": "20GiB"}},
                "state": {
                    "status": "Running", "status_code": 103, "pid": 4242,
                    "memory": {"usage": 1073741824_i64, "total": 2147483648_i64},
                    "cpu": {"usage": 500000000_i64},
                    "network": {"eth0": {"addresses": [
                        {"address": "10.1.2.3", "family": "inet", "scope": "global"},
                        {"address": "fe80::1", "family": "inet6", "scope": "link"}
                    ], "hwaddr": "00:11:22:33:44:55", "state": "up", "type": "broadcast"}}
                },
                "snapshots": [{"name": "before-upgrade", "created_at": "2026-01-01T00:00:00Z", "stateful": false}]
            }]
        });
        let list: Vec<IncusInstance> = IncusService::metadata(v).unwrap();
        let inst = &list[0];
        let st = inst.state.as_ref().unwrap();
        assert_eq!(st.pid, Some(4242));
        assert_eq!(st.memory.as_ref().unwrap().total, 2_147_483_648_i64);
        let net = st.network.as_ref().unwrap().get("eth0").unwrap();
        assert_eq!(net.addresses[0].address, "10.1.2.3");
        assert_eq!(inst.snapshots.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn operation_id_reads_async_field() {
        let v = json!({"type": "async", "operation": "/1.0/operations/abc-123"});
        assert_eq!(
            IncusService::operation_id(&v).unwrap(),
            "/1.0/operations/abc-123"
        );
        let v = json!({"type": "sync"});
        assert_eq!(IncusService::operation_id(&v).unwrap(), "");
    }

    #[test]
    fn create_body_serializes_to_incus_shape() {
        let body = CreateInstanceBody {
            name: "test".to_string(),
            instance_type: "virtual-machine".to_string(),
            source: InstanceSource {
                source_type: "image".to_string(),
                server: Some("https://images.linuxcontainers.org".to_string()),
                protocol: Some("simplestreams".to_string()),
                alias: Some("ubuntu/24.04/cloud".to_string()),
            },
            config: HashMap::from([("limits.cpu".to_string(), "2".to_string())]),
            devices: HashMap::new(),
            profiles: vec!["default".to_string()],
            start: true,
        };
        let j = serde_json::to_value(&body).unwrap();
        assert_eq!(j["type"], "virtual-machine");
        assert_eq!(j["source"]["type"], "image");
        assert_eq!(j["source"]["alias"], "ubuntu/24.04/cloud");
        assert_eq!(j["start"], true);
        assert_eq!(j["config"]["limits.cpu"], "2");
        // empty devices map must be omitted, not sent as {}
        assert!(j.get("devices").is_none());
    }

    #[test]
    fn os_catalogue_ids_are_unique_and_kinds_valid() {
        let cat = os_catalogue();
        assert!(cat.len() >= 4);
        let mut ids: Vec<&str> = cat.iter().map(|o| o.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), cat.len(), "duplicate OS id");
        assert_eq!(cat.iter().filter(|o| o.recommended).count(), 1);
        for o in cat {
            assert!(!o.kinds.is_empty());
            for k in o.kinds {
                assert!(*k == "vm" || *k == "container");
            }
            assert!(o.alias.contains('/'));
        }
    }

    #[test]
    fn os_catalogue_windows_entries_are_iso_only_vms() {
        for o in os_catalogue().iter().filter(|o| o.iso_only) {
            assert_eq!(o.kinds, &["vm"]);
        }
        let win11 = os_catalogue().iter().find(|o| o.id == "windows-11").unwrap();
        assert!(win11.iso_only);
        let win10 = os_catalogue().iter().find(|o| o.id == "windows-10").unwrap();
        assert!(win10.iso_only);
        assert!(os_catalogue().iter().filter(|o| o.iso_only).count() >= 2);
    }

    #[test]
    fn audio_device_is_a_qemu_hda() {
        let d = audio_device();
        assert_eq!(d["type"], "qemu");
        assert_eq!(d["driver"], "intel-hda");
    }

    #[test]
    fn resolve_os_alias_falls_back_to_recommended() {
        assert_eq!(resolve_os_alias("ubuntu-24.04"), "ubuntu/24.04/cloud");
        assert_eq!(resolve_os_alias("debian-12"), "debian/12/cloud");
        // unknown -> the single recommended entry
        assert_eq!(resolve_os_alias("nonsense"), "ubuntu/24.04/cloud");
    }

    #[test]
    fn cloud_init_hashes_password_and_never_stores_plaintext() {
        let ud = build_cloud_init("alice", "s3cr3t-p@ss");
        assert!(ud.starts_with("#cloud-config\n"));
        assert!(ud.contains("name: alice"));
        assert!(ud.contains("ssh_pwauth: true"));
        assert!(
            ud.contains("passwd: \"$6$"),
            "expected a SHA-512 crypt hash"
        );
        assert!(
            !ud.contains("s3cr3t-p@ss"),
            "plaintext password leaked into cloud-init"
        );
        // the hash must actually verify
        let hash = ud
            .lines()
            .find_map(|l| l.trim().strip_prefix("passwd: \""))
            .map(|s| s.trim_end_matches('"'))
            .unwrap();
        assert!(sha_crypt::ShaCrypt::SHA512
            .verify_password(
                b"s3cr3t-p@ss",
                sha_crypt::PasswordHashRef::new(hash).expect("valid MCF hash")
            )
            .is_ok());
    }

    #[test]
    fn incus_error_status_codes() {
        assert_eq!(IncusError::NotAvailable.status_code(), 503);
        assert_eq!(IncusError::InstanceNotFound("x".into()).status_code(), 404);
        assert_eq!(IncusError::Http(409, "taken".into()).status_code(), 409);
        assert_eq!(IncusError::Operation("boom".into()).status_code(), 500);
    }

    #[test]
    fn instance_state_tolerates_missing_optional_fields() {
        let st: IncusInstanceState = serde_json::from_value(json!({
            "status": "Stopped", "status_code": 102
        }))
        .unwrap();
        assert_eq!(st.status_code, 102);
        assert!(st.memory.is_none());
        assert!(st.network.is_none());
    }

    #[test]
    fn instance_state_tolerates_explicit_nulls() {
        // A freshly-booted VM: Incus sends `"disk": null` (not absent). Plain
        // `#[serde(default)]` would fail here — `null_default` must save it.
        let st: IncusInstanceState = serde_json::from_value(json!({
            "status": "Running", "status_code": 103, "pid": 123,
            "disk": null, "memory": null, "network": null, "cpu": null
        }))
        .unwrap();
        assert_eq!(st.status_code, 103);
        assert!(st.disk.is_empty());
        assert!(st.memory.is_none());

        let inst: IncusInstance = serde_json::from_value(json!({
            "name": "iso-vm", "type": "virtual-machine", "status": "Running", "status_code": 103,
            "config": null, "devices": null, "expanded_devices": null, "profiles": null,
            "state": { "status": "Running", "status_code": 103, "disk": null }
        }))
        .unwrap();
        assert!(inst.config.is_empty());
        assert!(inst.state.unwrap().disk.is_empty());
    }
}
