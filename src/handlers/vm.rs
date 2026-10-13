//! HTTP handlers for the **Virtual Machines** app (Incus-backed).
//!
//! Design goal: a person with no Linux knowledge can create a machine, log
//! into it, snapshot it and throw it away. So the surface is small and every
//! choice has a sane default:
//!
//! * `GET  /api/vm/available`            — is Incus usable?
//! * `GET  /api/vm/os-options`           — the curated OS list for the wizard
//! * `GET  /api/vm/list`                 — every machine, with live status/IP
//! * `POST /api/vm/create`               — name + OS + size + login → machine
//! * `GET  /api/vm/{name}`               — one machine, full detail
//! * `DELETE /api/vm/{name}`             — stop (if needed) then delete
//! * `PUT  /api/vm/{name}/start|stop|restart`
//! * `PUT  /api/vm/{name}/config`        — change CPU / memory
//! * `GET|POST /api/vm/{name}/snapshots`
//! * `POST /api/vm/{name}/snapshots/{snap}/restore`
//! * `DELETE /api/vm/{name}/snapshots/{snap}`
//! * `POST /api/vm/console/ticket` + `GET /api/vm/console/ws` — shell console
//!
//! Every endpoint is **admin-only** (Incus talks to a root socket and hands
//! out disk/network resources). Long Incus operations are awaited so the UI
//! never lies about state; `create` is the exception — it runs as a tracked
//! background job (image downloads take minutes).

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::incus::{self, ExecSession, IncusError, IncusInstance, IncusService, STATUS_RUNNING};
use crate::sse::{JobType, PROGRESS_MANAGER};
use crate::utils::ResponseHelper;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message as TMessage;

// ── Tunables ────────────────────────────────────────────────────────────────

/// How long a lifecycle call (start / stop / restart / delete / restore) waits
/// for Incus before giving up and reporting what it knows.
const LIFECYCLE_WAIT_SECS: u64 = 40;
/// Ceiling for a `create` job (first Ubuntu image pull on a slow line).
const CREATE_WAIT_SECS: u64 = 900;
/// Console ticket lifetime — just long enough for the browser to open the WS.
const TICKET_TTL: Duration = Duration::from_secs(20);
/// Custom Incus config key where we stash the seeded login name so the detail
/// view can tell the person how to connect.
const LOGIN_KEY: &str = "user.homevault-login";
/// Custom Incus config key holding the VNC display number for an ISO-install
/// VM (the graphical screen). The VNC server itself is added via `raw.qemu`
/// on `127.0.0.1:5900+display` and never leaves loopback.
const VNC_KEY: &str = "user.homevault-vnc";
/// VNC display range we hand out (port 5900+N). Kept away from the classic
/// 5900-5910 range so it does not clash with a desktop VNC server.
const VNC_DISPLAY_MIN: u16 = 20;
const VNC_DISPLAY_MAX: u16 = 119;

// ── Size presets ────────────────────────────────────────────────────────────

/// (cpu cores, memory MiB, disk GiB) for the three wizard buttons.
fn size_preset(name: &str) -> (u64, u64, u64) {
    match name {
        "small" => (1, 1024, 10),
        "large" => (4, 8192, 60),
        _ => (2, 2048, 25), // "medium" / anything else
    }
}

// ── Request bodies ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateVmRequest {
    pub name: String,
    /// "vm" (default) or "container". Forced to "vm" for an ISO install.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// OS catalogue id, e.g. `ubuntu-24.04`. Empty when installing from an ISO.
    #[serde(default)]
    pub os: String,
    /// "small" | "medium" | "large" — ignored when explicit specs are given.
    #[serde(default = "default_size")]
    pub size: String,
    #[serde(default)]
    pub cpu_cores: Option<u64>,
    #[serde(default)]
    pub memory_mb: Option<u64>,
    #[serde(default)]
    pub disk_gb: Option<u64>,
    /// Login account seeded into the machine (catalogue installs only).
    #[serde(default = "default_login")]
    pub username: String,
    /// Password for that account. Required for a catalogue install; ignored
    /// for an ISO install (the person sets their account inside the installer).
    #[serde(default)]
    pub password: String,

    // ── ISO install ────────────────────────────────────────────────────────
    /// Absolute path to a local `.iso` to boot from. When set, this is an ISO
    /// install: an empty VM with a graphical screen, no seeded account.
    #[serde(default)]
    pub iso_path: Option<String>,
    /// `"windows"` applies the Windows-friendly hardware (NVMe system disk so
    /// Setup sees it without extra drivers, a TPM 2.0, Secure Boot on).
    /// Anything else / omitted → a generic UEFI Linux VM.
    #[serde(default)]
    pub iso_os: Option<String>,
    /// Optional second CD-ROM — the VirtIO driver `.iso` (Windows network /
    /// balloon drivers). Only used when `iso_os` is `"windows"`.
    #[serde(default)]
    pub drivers_iso_path: Option<String>,
    /// Override Secure Boot (Windows defaults on, Linux defaults off for the
    /// widest ISO compatibility).
    #[serde(default)]
    pub secure_boot: Option<bool>,
    /// Attach a virtual sound card (VMs only, ignored for containers).
    #[serde(default)]
    pub audio: Option<bool>,
}

fn default_kind() -> String {
    "vm".to_string()
}
fn default_size() -> String {
    "medium".to_string()
}
fn default_login() -> String {
    "admin".to_string()
}

#[derive(Deserialize)]
pub struct ConfigRequest {
    pub cpu_cores: Option<u64>,
    pub memory_mb: Option<u64>,
}

#[derive(Deserialize)]
pub struct SnapshotRequest {
    pub name: String,
    #[serde(default)]
    pub stateful: bool,
}

#[derive(Deserialize)]
pub struct ConsoleTicketRequest {
    pub name: String,
    #[serde(default)]
    pub cols: Option<u32>,
    #[serde(default)]
    pub rows: Option<u32>,
}

#[derive(Deserialize)]
pub struct ConsoleWsQuery {
    pub ticket: String,
}

// ── Response shapes (the one wire contract the frontend mirrors) ────────────

#[derive(Serialize)]
pub struct VmInfo {
    pub name: String,
    /// "vm" | "container"
    pub kind: String,
    /// "running" | "stopped" | "starting" | "stopping" | "error" | "unknown"
    pub status: String,
    pub running: bool,
    pub cpu_cores: Option<u64>,
    pub memory_mb: Option<u64>,
    pub disk_gb: Option<u64>,
    pub ipv4: Option<String>,
    pub os: Option<String>,
    pub created_at: String,
    pub snapshot_count: usize,
    /// This VM has a graphical screen (it was installed from an ISO). The
    /// frontend shows "Open screen" instead of "Open console".
    pub has_screen: bool,
}

#[derive(Serialize)]
pub struct VmDetail {
    #[serde(flatten)]
    pub info: VmInfo,
    pub login_user: Option<String>,
    pub memory_used_mb: Option<u64>,
    pub disk_used_gb: Option<f64>,
    pub snapshots: Vec<SnapshotInfo>,
}

#[derive(Serialize)]
pub struct SnapshotInfo {
    pub name: String,
    pub created_at: String,
    pub stateful: bool,
}

#[derive(Serialize)]
pub struct AvailableInfo {
    pub available: bool,
}

#[derive(Serialize)]
pub struct TicketInfo {
    pub ticket: String,
}

// ── Console / screen ticket store ──────────────────────────────────────────
//
// One single-use, short-TTL token per attempt to open a shell console or a
// graphical screen. Keeps a long-lived credential out of the WebSocket URL.

#[derive(Clone, Copy, PartialEq)]
enum TicketKind {
    /// Interactive `exec` shell — carries the requested terminal size.
    Shell { cols: u32, rows: u32 },
    /// Graphical VNC screen (ISO-install VMs).
    Screen,
}

struct Ticket {
    username: String,
    vm: String,
    kind: TicketKind,
    issued: Instant,
}

static TICKETS: LazyLock<RwLock<HashMap<String, Ticket>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn random_token(len: usize) -> String {
    use rand::RngExt;
    const CS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CS[rng.random_range(0..CS.len())] as char)
        .collect()
}

async fn issue_ticket(username: &str, vm: &str, kind: TicketKind) -> String {
    let token = random_token(32);
    let mut map = TICKETS.write().await;
    let now = Instant::now();
    map.retain(|_, t| now.duration_since(t.issued) < TICKET_TTL);
    map.insert(
        token.clone(),
        Ticket {
            username: username.to_string(),
            vm: vm.to_string(),
            kind,
            issued: now,
        },
    );
    token
}

async fn consume_ticket(token: &str) -> Option<(String, String, TicketKind)> {
    let mut map = TICKETS.write().await;
    let t = map.remove(token)?;
    if t.issued.elapsed() >= TICKET_TTL {
        return None;
    }
    Some((t.username, t.vm, t.kind))
}

// ── Router (console + screen WebSockets; REST is wired in web/mod.rs) ───────

pub fn create_vm_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/console/ws", get(handle_console_ws))
        .route("/screen/ws", get(handle_screen_ws))
        .with_state(state)
}

/// Displays handed out in the last minute — so two near-simultaneous creates
/// don't both grab the same free port before either VM's qemu binds it.
static RESERVED_DISPLAYS: LazyLock<parking_lot::Mutex<HashMap<u16, Instant>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// Find a free VNC display (`127.0.0.1:5900+N`) for a new ISO-install VM:
/// its loopback port must be bindable *and* not just handed to another create.
fn alloc_vnc_display() -> Option<u16> {
    use std::net::TcpListener;
    let mut reserved = RESERVED_DISPLAYS.lock();
    let now = Instant::now();
    reserved.retain(|_, t| now.duration_since(*t) < Duration::from_secs(60));
    let pick = (VNC_DISPLAY_MIN..=VNC_DISPLAY_MAX).find(|d| {
        !reserved.contains_key(d) && TcpListener::bind(("127.0.0.1", 5900 + d)).is_ok()
    })?;
    reserved.insert(pick, now);
    Some(pick)
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/vm/available
pub async fn handle_vm_available(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    ResponseHelper::json_success(AvailableInfo {
        available: IncusService::is_available().await,
    })
}

/// GET /api/vm/os-options
pub async fn handle_vm_os_options(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    ResponseHelper::json_success(incus::os_catalogue())
}

/// GET /api/vm/list
pub async fn handle_vm_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    match IncusService::list_instances().await {
        Ok(list) => {
            let vms: Vec<VmInfo> = list.iter().map(instance_to_info).collect();
            ResponseHelper::json_success(vms)
        }
        Err(e) => incus_err(e),
    }
}

/// GET /api/vm/{name}
pub async fn handle_vm_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    let inst = match IncusService::get_instance(&name).await {
        Ok(i) => i,
        Err(e) => return incus_err(e),
    };
    let snaps = IncusService::list_snapshots(&name)
        .await
        .unwrap_or_default();

    let mut info = instance_to_info(&inst);
    info.snapshot_count = snaps.len();

    let mem_used = inst
        .state
        .as_ref()
        .and_then(|s| s.memory.as_ref())
        .map(|m| (m.usage.max(0) as u64) / (1024 * 1024))
        .filter(|v| *v > 0);
    let disk_used = inst
        .state
        .as_ref()
        .and_then(|s| s.disk.get("root"))
        .map(|d| (d.usage.max(0) as f64) / 1_073_741_824.0)
        .filter(|v| *v > 0.0);

    ResponseHelper::json_success(VmDetail {
        info,
        login_user: inst
            .config
            .get(LOGIN_KEY)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        memory_used_mb: mem_used,
        disk_used_gb: disk_used,
        snapshots: snaps
            .into_iter()
            .map(|s| SnapshotInfo {
                name: s.name,
                created_at: s.created_at,
                stateful: s.stateful,
            })
            .collect(),
    })
}

/// POST /api/vm/create
pub async fn handle_vm_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateVmRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    // ---- validation, all in plain-language errors ----
    let name = req.name.trim().to_lowercase();
    if !valid_instance_name(&name) {
        return ResponseHelper::json_error(
            "The name must be 1–63 characters: lowercase letters, digits and dashes, starting and ending with a letter or digit.".to_string(),
        );
    }

    let iso = req
        .iso_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let from_catalogue = !req.os.trim().is_empty();
    if iso.is_some() && from_catalogue {
        return ResponseHelper::json_error(
            "Choose either a ready-made system or an installation ISO — not both.".to_string(),
        );
    }
    if iso.is_none() && !from_catalogue {
        return ResponseHelper::json_error(
            "Pick a system, or choose an installation ISO.".to_string(),
        );
    }

    // Machine already there?
    if IncusService::get_instance(&name).await.is_ok() {
        return ResponseHelper::json_error(format!(
            "A machine called \"{}\" already exists.",
            name
        ));
    }

    let (mut cpu, mut mem, mut disk) = size_preset(&req.size);

    // ── Build the create body — one of two shapes ─────────────────────────
    // `tpm_note` is Some(msg) when a Windows VM had to be created without a TPM
    // up front; `has_removable_tpm` means a `vtpm` device was attached and can
    // be dropped if it turns out this host's Incus can't back it.
    let (body, job_label, tpm_note, has_removable_tpm): (
        incus::CreateInstanceBody,
        String,
        Option<String>,
        bool,
    ) = if let Some(iso_path) = iso {
        // ---------- ISO install ----------
        let iso_path = match validate_local_iso(iso_path) {
            Ok(p) => p,
            Err(e) => return ResponseHelper::json_error(e),
        };
        let windows = req.iso_os.as_deref() == Some("windows");
        let drivers = match req
            .drivers_iso_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(p) => match validate_local_iso(p) {
                Ok(p) => Some(p),
                Err(e) => return ResponseHelper::json_error(format!("Driver ISO: {e}")),
            },
            None => None,
        };

        // Windows needs more room than the Linux presets.
        if windows {
            let (c, m, d) = match req.size.as_str() {
                "small" => (2, 4096, 40),
                "large" => (6, 12288, 100),
                _ => (4, 8192, 64),
            };
            cpu = c;
            mem = m;
            disk = d;
        }
        if let Some(c) = req.cpu_cores {
            cpu = c.clamp(1, 64);
        }
        if let Some(m) = req.memory_mb {
            mem = m.clamp(1024, 131_072);
        }
        if let Some(d) = req.disk_gb {
            disk = d.clamp(10, 4096);
        }

        // Secure Boot: default on for Windows (Win11 requires it), off for
        // Linux. Refuse "on" when the host has no Microsoft-signed firmware —
        // the guest would just fail to boot with no explanation.
        let secure_boot = req.secure_boot.unwrap_or(windows);
        if secure_boot && !ovmf_secureboot_available() {
            return ResponseHelper::json_error(
                "Secure Boot needs the Microsoft-signed UEFI firmware, which is not installed on this server (package `ovmf`). Install it, or turn Secure Boot off in the advanced options.".to_string(),
            );
        }

        let display = match alloc_vnc_display() {
            Some(d) => d,
            None => {
                return ResponseHelper::json_error(
                    "The server has no free display slot for another VM screen.".to_string(),
                )
            }
        };

        let mut config: HashMap<String, String> = HashMap::new();
        config.insert("limits.cpu".to_string(), cpu.to_string());
        config.insert("limits.memory".to_string(), format!("{}MiB", mem));
        config.insert(
            "raw.qemu".to_string(),
            format!("-vnc 127.0.0.1:{},share=force-shared", display),
        );
        config.insert(VNC_KEY.to_string(), display.to_string());
        config.insert("security.secureboot".to_string(), secure_boot.to_string());

        let mut devices: HashMap<String, serde_json::Value> = HashMap::new();
        devices.insert(
            "root".to_string(),
            if windows {
                serde_json::json!({
                    "type": "disk", "path": "/", "pool": "default",
                    "size": format!("{}GiB", disk),
                    // NVMe: Windows Setup sees the system disk with no extra driver.
                    "io.bus": "nvme",
                })
            } else {
                serde_json::json!({
                    "type": "disk", "path": "/", "pool": "default",
                    "size": format!("{}GiB", disk),
                })
            },
        );
        devices.insert(
            "install-media".to_string(),
            serde_json::json!({
                "type": "disk", "source": iso_path, "boot.priority": "10",
            }),
        );
        if let Some(drv) = drivers {
            devices.insert(
                "virtio-drivers".to_string(),
                serde_json::json!({ "type": "disk", "source": drv }),
            );
        }
        // TPM 2.0 for Windows 11 — only if the host can actually provide it
        // (`swtpm-tools`). If not, the VM is still created (Windows 10 is fine,
        // Win11 can be bypassed) and the job note says so.
        let tpm_wanted = windows;
        let tpm_added = tpm_wanted && swtpm_available();
        if tpm_added {
            devices.insert("vtpm".to_string(), serde_json::json!({ "type": "tpm" }));
        }
        apply_audio_device(&mut devices, req.audio.unwrap_or(false), true);

        let label = if windows {
            "Windows (ISO)"
        } else {
            "Linux (ISO)"
        };
        let tpm_note = if tpm_wanted && !tpm_added {
            Some("created without a TPM 2.0 — Windows 11 needs one; install `swtpm-tools` (or update Incus) and recreate.".to_string())
        } else {
            None
        };
        (
            incus::CreateInstanceBody {
                name: name.clone(),
                instance_type: "virtual-machine".to_string(),
                source: incus::InstanceSource {
                    source_type: "none".to_string(),
                    server: None,
                    protocol: None,
                    alias: None,
                },
                config,
                devices,
                profiles: vec!["default".to_string()],
                start: true,
            },
            label.to_string(),
            tpm_note,
            tpm_added,
        )
    } else {
        // ---------- catalogue install ----------
        let kind = match req.kind.as_str() {
            "vm" | "virtual-machine" => "virtual-machine",
            "container" => "container",
            _ => {
                return ResponseHelper::json_error(
                    "Choose a virtual machine or a container.".to_string(),
                )
            }
        };
        let os = match os_or_error(&req.os) {
            Ok(o) => o,
            Err(e) => return e,
        };
        if os.iso_only {
            return ResponseHelper::json_error(format!(
                "{} has no ready-made image — install it from your ISO instead.",
                os.label,
            ));
        }
        if !os.kinds.contains(&if kind == "virtual-machine" {
            "vm"
        } else {
            "container"
        }) {
            return ResponseHelper::json_error(format!(
                "{} is only available as a {}.",
                os.label,
                if os.kinds.contains(&"vm") {
                    "virtual machine"
                } else {
                    "container"
                }
            ));
        }
        let username = req.username.trim().to_lowercase();
        if !valid_unix_username(&username) {
            return ResponseHelper::json_error(
                "The login name must be 1–32 characters: lowercase letters, digits, dash or underscore.".to_string(),
            );
        }
        if req.password.len() < 6 || req.password.len() > 128 {
            return ResponseHelper::json_error(
                "The password must be between 6 and 128 characters.".to_string(),
            );
        }

        if let Some(c) = req.cpu_cores {
            cpu = c.clamp(1, 64);
        }
        if let Some(m) = req.memory_mb {
            mem = m.clamp(256, 131_072);
        }
        if let Some(d) = req.disk_gb {
            disk = d.clamp(5, 4096);
        }

        let mut config: HashMap<String, String> = HashMap::new();
        config.insert("limits.cpu".to_string(), cpu.to_string());
        config.insert("limits.memory".to_string(), format!("{}MiB", mem));
        config.insert(
            "cloud-init.user-data".to_string(),
            incus::build_cloud_init(&username, &req.password),
        );
        config.insert(LOGIN_KEY.to_string(), username.clone());

        let mut devices: HashMap<String, serde_json::Value> = HashMap::new();
        devices.insert(
            "root".to_string(),
            serde_json::json!({
                "type": "disk", "path": "/", "pool": "default",
                "size": format!("{}GiB", disk),
            }),
        );
        apply_audio_device(
            &mut devices,
            req.audio.unwrap_or(false),
            kind == "virtual-machine",
        );
        (
            incus::CreateInstanceBody {
                name: name.clone(),
                instance_type: kind.to_string(),
                source: incus::InstanceSource {
                    source_type: "image".to_string(),
                    server: Some("https://images.linuxcontainers.org".to_string()),
                    protocol: Some("simplestreams".to_string()),
                    alias: Some(os.alias.to_string()),
                },
                config,
                devices,
                profiles: vec!["default".to_string()],
                start: true,
            },
            os.label.to_string(),
            None,
            false,
        )
    };

    let op = match IncusService::create_instance(&body).await {
        Ok(op) => op,
        Err(e) => return incus_err(e),
    };

    // Track the (possibly long) image download + first boot as a desktop job.
    let job = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("vm_create".to_string()),
        &format!("Creating \"{}\" ({})", name, job_label),
        Some(admin.clone()),
    );
    let job_name = name.clone();
    let retry_body = body.clone();
    tokio::spawn(async move {
        let ready_msg = |extra: Option<&str>| match extra {
            Some(n) => format!("\"{}\" is ready to use — {}", job_name, n),
            None => format!("\"{}\" is ready to use", job_name),
        };
        if op.is_empty() {
            PROGRESS_MANAGER.complete_job(&job, &ready_msg(tpm_note.as_deref()));
            crate::events::log_event(format!(
                "VM '{}' ({}) created by {}",
                job_name, job_label, admin
            ));
            return;
        }
        let mut outcome = IncusService::wait_for_operation(&op, CREATE_WAIT_SECS).await;

        // Self-heal: on an old Incus the emulated-TPM wiring is broken and the
        // start fails with an opaque error. If we attached a `vtpm`, wipe the
        // failed instance and recreate it without one — the person gets a
        // working (Windows-10-capable) VM instead of nothing.
        if outcome.is_err() && has_removable_tpm {
            // The failed instance must be fully gone before we can reuse the name.
            if let Ok(dop) = IncusService::delete_instance(&job_name).await {
                if !dop.is_empty() {
                    let _ = IncusService::wait_for_operation(&dop, LIFECYCLE_WAIT_SECS).await;
                }
            }
            let mut b2 = retry_body.clone();
            b2.devices.remove("vtpm");
            // Another VM may have taken this display while we were failing —
            // grab a fresh one for the retry.
            if let Some(fresh) = alloc_vnc_display() {
                b2.config.insert(
                    "raw.qemu".to_string(),
                    format!("-vnc 127.0.0.1:{},share=force-shared", fresh),
                );
                b2.config.insert(VNC_KEY.to_string(), fresh.to_string());
            }
            if let Ok(op2) = IncusService::create_instance(&b2).await {
                if IncusService::wait_for_operation(&op2, CREATE_WAIT_SECS)
                    .await
                    .is_ok()
                {
                    PROGRESS_MANAGER.complete_job(
                        &job,
                        &format!(
                            "\"{}\" is ready — but without a TPM 2.0 (Windows 11 needs one; this server's Incus is too old to provide it).",
                            job_name
                        ),
                    );
                    crate::events::log_event(format!(
                        "VM '{}' ({}) created by {} (TPM dropped — host can't back it)",
                        job_name, job_label, admin
                    ));
                    return;
                }
            }
            outcome = Err(IncusError::Operation(
                "the machine could not start".to_string(),
            ));
        }

        match outcome {
            Ok(()) => {
                PROGRESS_MANAGER.complete_job(&job, &ready_msg(tpm_note.as_deref()));
                crate::events::log_event(format!(
                    "VM '{}' ({}) created by {}",
                    job_name, job_label, admin
                ));
            }
            Err(e) => {
                let reason = e.to_string();
                let friendly = if reason.to_lowercase().contains("image") {
                    format!(
                        "Could not create \"{}\": the {} image could not be downloaded. Try again, or pick another system.",
                        job_name, job_label
                    )
                } else {
                    format!("Could not create \"{}\": {}", job_name, reason)
                };
                PROGRESS_MANAGER.fail_job(&job, &friendly);
                let _ = IncusService::delete_instance(&job_name).await;
                crate::events::log_event_warn(format!(
                    "VM '{}' creation failed for {}: {}",
                    job_name, admin, e
                ));
            }
        }
    });

    ResponseHelper::json_success(serde_json::json!({
        "name": name,
        "status": "creating",
    }))
}

/// DELETE /api/vm/{name}
pub async fn handle_vm_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    let inst = match IncusService::get_instance(&name).await {
        Ok(i) => i,
        Err(e) => return incus_err(e),
    };

    // Must be stopped first — do it for the person.
    if inst.status_code == STATUS_RUNNING {
        if let Ok(op) = IncusService::set_state(&name, "stop", true, Some(20)).await {
            let _ = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await;
        }
    }

    match IncusService::delete_instance(&name).await {
        Ok(op) => {
            if !op.is_empty() {
                if let Err(e) = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await {
                    return incus_err(e);
                }
            }
            crate::events::log_event(format!("VM '{}' deleted by {}", name, admin));
            ResponseHelper::json_success(format!("\"{}\" was deleted.", name))
        }
        Err(e) => incus_err(e),
    }
}

/// PUT /api/vm/{name}/start
pub async fn handle_vm_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    lifecycle(&state, &headers, &name, "start", false).await
}

/// PUT /api/vm/{name}/stop
pub async fn handle_vm_stop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    lifecycle(&state, &headers, &name, "stop", false).await
}

/// PUT /api/vm/{name}/restart
pub async fn handle_vm_restart(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    lifecycle(&state, &headers, &name, "restart", false).await
}

async fn lifecycle(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    name: &str,
    action: &str,
    _force: bool,
) -> Response {
    let admin = match require_admin(state, headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    // A screen VM bakes its VNC display into `raw.qemu` at create time. If that
    // loopback port has since been taken by something else, qemu won't start —
    // move the display to a free one before starting a *stopped* VM. (Skip for
    // a running VM/restart: its own qemu holds the port, which is expected.)
    if action == "start" {
        if let Ok(inst) = IncusService::get_instance(name).await {
            if inst.status_code != STATUS_RUNNING {
                if let Some(display) = vnc_display(&inst) {
                    use std::net::TcpListener;
                    if TcpListener::bind(("127.0.0.1", 5900 + display)).is_err() {
                        if let Some(fresh) = alloc_vnc_display() {
                            let mut c = HashMap::new();
                            c.insert(
                                "raw.qemu".to_string(),
                                format!("-vnc 127.0.0.1:{},share=force-shared", fresh),
                            );
                            c.insert(VNC_KEY.to_string(), fresh.to_string());
                            let _ = IncusService::update_config(name, c).await;
                        }
                    }
                }
            }
        }
    }

    let timeout = if action == "start" { None } else { Some(30) };
    let first = IncusService::set_state(name, action, false, timeout).await;
    let op = match first {
        Ok(op) => op,
        Err(e) => return incus_err(e),
    };

    let mut result = if op.is_empty() {
        Ok(())
    } else {
        IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await
    };

    // A stubborn guest that ignored the ACPI request — hit it harder once.
    if result.is_err() && (action == "stop" || action == "restart") {
        if let Ok(op) = IncusService::set_state(name, action, true, Some(15)).await {
            result = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await;
        }
    }

    match result {
        Ok(()) => {
            crate::events::log_event(format!("VM '{}' {}ed by {}", name, action, admin));
            let msg = match action {
                "start" => format!("\"{}\" is starting.", name),
                "stop" => format!("\"{}\" was stopped.", name),
                _ => format!("\"{}\" is restarting.", name),
            };
            ResponseHelper::json_success(msg)
        }
        Err(e) => incus_err(e),
    }
}

/// PUT /api/vm/{name}/config
pub async fn handle_vm_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(req): Json<ConfigRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    let mut config = HashMap::new();
    if let Some(cpu) = req.cpu_cores {
        if !(1..=64).contains(&cpu) {
            return ResponseHelper::json_error("CPU cores must be between 1 and 64.".to_string());
        }
        config.insert("limits.cpu".to_string(), cpu.to_string());
    }
    if let Some(mem) = req.memory_mb {
        if !(256..=131_072).contains(&mem) {
            return ResponseHelper::json_error(
                "Memory must be between 256 MB and 128 GB.".to_string(),
            );
        }
        config.insert("limits.memory".to_string(), format!("{}MiB", mem));
    }
    if config.is_empty() {
        return ResponseHelper::json_error("Nothing to change.".to_string());
    }

    match IncusService::update_config(&name, config).await {
        Ok(()) => {
            crate::events::log_event(format!("VM '{}' resized by {}", name, admin));
            ResponseHelper::json_success(
                "Saved. Restart the machine for the change to take effect.".to_string(),
            )
        }
        Err(e) => incus_err(e),
    }
}

/// GET /api/vm/{name}/snapshots
pub async fn handle_vm_snapshots(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    match IncusService::list_snapshots(&name).await {
        Ok(list) => ResponseHelper::json_success(
            list.into_iter()
                .map(|s| SnapshotInfo {
                    name: s.name,
                    created_at: s.created_at,
                    stateful: s.stateful,
                })
                .collect::<Vec<_>>(),
        ),
        Err(e) => incus_err(e),
    }
}

/// POST /api/vm/{name}/snapshots
pub async fn handle_vm_snapshot_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(req): Json<SnapshotRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let snap = req.name.trim();
    if !valid_snapshot_name(snap) {
        return ResponseHelper::json_error(
            "The snapshot name may only contain letters, digits, dots, dashes and underscores."
                .to_string(),
        );
    }
    match IncusService::create_snapshot(&name, snap, req.stateful).await {
        Ok(op) => {
            if !op.is_empty() {
                if let Err(e) = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await {
                    return incus_err(e);
                }
            }
            crate::events::log_event(format!(
                "Snapshot '{}' of VM '{}' created by {}",
                snap, name, admin
            ));
            ResponseHelper::json_success(format!("Saved the state as \"{}\".", snap))
        }
        Err(e) => incus_err(e),
    }
}

/// POST /api/vm/{name}/snapshots/{snap}/restore
pub async fn handle_vm_snapshot_restore(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((name, snap)): Path<(String, String)>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    let inst = match IncusService::get_instance(&name).await {
        Ok(i) => i,
        Err(e) => return incus_err(e),
    };
    let was_running = inst.status_code == STATUS_RUNNING;

    if was_running {
        if let Ok(op) = IncusService::set_state(&name, "stop", true, Some(20)).await {
            let _ = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await;
        }
    }

    let restore = IncusService::restore_snapshot(&name, &snap).await;
    let outcome = match restore {
        Ok(op) if !op.is_empty() => {
            IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    };

    // Put it back the way we found it.
    if was_running {
        if let Ok(op) = IncusService::set_state(&name, "start", false, None).await {
            let _ = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await;
        }
    }

    match outcome {
        Ok(()) => {
            crate::events::log_event(format!(
                "VM '{}' restored to snapshot '{}' by {}",
                name, snap, admin
            ));
            ResponseHelper::json_success(format!("\"{}\" was rolled back to \"{}\".", name, snap))
        }
        Err(e) => incus_err(e),
    }
}

/// DELETE /api/vm/{name}/snapshots/{snap}
pub async fn handle_vm_snapshot_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((name, snap)): Path<(String, String)>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    match IncusService::delete_snapshot(&name, &snap).await {
        Ok(op) => {
            if !op.is_empty() {
                let _ = IncusService::wait_for_operation(&op, LIFECYCLE_WAIT_SECS).await;
            }
            crate::events::log_event(format!(
                "Snapshot '{}' of VM '{}' deleted by {}",
                snap, name, admin
            ));
            ResponseHelper::json_success(format!("Deleted \"{}\".", snap))
        }
        Err(e) => incus_err(e),
    }
}

/// POST /api/vm/console/ticket
pub async fn handle_console_ticket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ConsoleTicketRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    match IncusService::get_instance(&req.name).await {
        Ok(inst) if inst.status_code == STATUS_RUNNING => {}
        Ok(_) => {
            return ResponseHelper::json_error(
                "Start the machine before opening its console.".to_string(),
            )
        }
        Err(e) => return incus_err(e),
    }

    let cols = req.cols.unwrap_or(100).clamp(20, 500);
    let rows = req.rows.unwrap_or(30).clamp(5, 300);
    let ticket = issue_ticket(&admin, &req.name, TicketKind::Shell { cols, rows }).await;
    ResponseHelper::json_success(TicketInfo { ticket })
}

/// GET /api/vm/console/ws?ticket=…
pub async fn handle_console_ws(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ConsoleWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((username, vm, kind)) = consume_ticket(&q.ticket).await else {
        return ResponseHelper::json_unauthorized("Invalid or expired console ticket.".to_string());
    };
    let TicketKind::Shell { cols, rows } = kind else {
        return ResponseHelper::json_error("Wrong ticket type for the console.".to_string());
    };
    // Defence in depth — the ticket was only ever handed to an admin, but
    // re-check at connect time (same as the Terminal app).
    if !state.auth.is_admin(&username).await {
        return ResponseHelper::json_forbidden("Admin only.".to_string());
    }
    ws.on_upgrade(move |socket| console_relay(socket, username, vm, cols, rows))
}

/// POST /api/vm/{name}/screen/ticket — issue a VNC screen ticket for an
/// ISO-install VM that is running.
pub async fn handle_screen_ticket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };

    let inst = match IncusService::get_instance(&name).await {
        Ok(i) => i,
        Err(e) => return incus_err(e),
    };
    if vnc_display(&inst).is_none() {
        return ResponseHelper::json_error("This machine has no graphical screen.".to_string());
    }
    if inst.status_code != STATUS_RUNNING {
        return ResponseHelper::json_error("Start the machine to open its screen.".to_string());
    }

    let ticket = issue_ticket(&admin, &name, TicketKind::Screen).await;
    ResponseHelper::json_success(TicketInfo { ticket })
}

/// GET /api/vm/screen/ws?ticket=… — relay the browser (noVNC) to the VM's
/// loopback VNC server.
pub async fn handle_screen_ws(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ConsoleWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((username, vm, kind)) = consume_ticket(&q.ticket).await else {
        return ResponseHelper::json_unauthorized("Invalid or expired screen ticket.".to_string());
    };
    if kind != TicketKind::Screen {
        return ResponseHelper::json_error("Wrong ticket type for the screen.".to_string());
    }
    if !state.auth.is_admin(&username).await {
        return ResponseHelper::json_forbidden("Admin only.".to_string());
    }

    let display = match IncusService::get_instance(&vm).await {
        Ok(inst) => match vnc_display(&inst) {
            Some(d) if inst.status_code == STATUS_RUNNING => d,
            _ => {
                return ResponseHelper::json_error(
                    "The machine's screen is not available.".to_string(),
                )
            }
        },
        Err(e) => return incus_err(e),
    };

    ws.protocols(["binary"])
        .on_upgrade(move |socket| screen_relay(socket, username, vm, display))
}

/// Read the stored VNC display number off an instance's config.
fn vnc_display(inst: &IncusInstance) -> Option<u16> {
    inst.config
        .get(VNC_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| s.trim().parse::<u16>().ok())
}

/// Bridge a browser WebSocket (noVNC, raw RFB in binary frames) to the VM's
/// `127.0.0.1:5900+display` VNC server. Pure byte pipe, both directions.
async fn screen_relay(browser: WebSocket, username: String, vm: String, display: u16) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    crate::events::log_event(format!("Screen opened on VM '{}' by {}", vm, username));

    let tcp = match tokio::net::TcpStream::connect(("127.0.0.1", 5900 + display)).await {
        Ok(s) => s,
        Err(e) => {
            let (mut tx, _rx) = browser.split();
            let _ = tx
                .send(Message::Text(format!("screen unavailable: {e}").into()))
                .await;
            let _ = tx.close().await;
            return;
        }
    };
    let (mut tcp_rd, mut tcp_wr) = tcp.into_split();
    let (mut ws_tx, mut ws_rx) = browser.split();

    // VNC server → browser
    let mut down = tokio::spawn(async move {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match tcp_rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.close().await;
    });

    // browser → VNC server
    let mut up = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(b) => {
                    if tcp_wr.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Message::Text(t) => {
                    if tcp_wr.write_all(t.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = tcp_wr.shutdown().await;
    });

    tokio::select! {
        _ = &mut down => up.abort(),
        _ = &mut up => down.abort(),
    }
    crate::events::log_event(format!("Screen on VM '{}' closed by {}", vm, username));
}

// ── Managed ISO storage (download from URL / VirtIO drivers) ───────────────

#[derive(Serialize)]
pub struct IsoEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub virtio: bool,
}

#[derive(Deserialize)]
pub struct IsoDownloadRequest {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Latest stable VirtIO drivers for Windows guests (Fedora's canonical mirror).
const VIRTIO_WIN_URL: &str =
    "https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso";

fn ensure_iso_dir() -> Result<std::path::PathBuf, String> {
    let d = crate::config::paths::vm_iso_dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("Could not create the ISO folder: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
        for p in d.ancestors().skip(1).take(3) {
            if let Ok(m) = std::fs::metadata(p) {
                let mode = m.permissions().mode();
                if mode & 0o001 == 0 {
                    let _ =
                        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode | 0o001));
                }
            }
        }
    }
    Ok(d)
}

fn safe_iso_name(n: &str) -> Option<String> {
    let n = n.trim();
    if n.is_empty() || n.len() > 128 || n.starts_with('.') {
        return None;
    }
    if n.contains('/') || n.contains('\\') || n.contains("..") || n.contains('\0') {
        return None;
    }
    if !n.to_ascii_lowercase().ends_with(".iso") {
        return None;
    }
    if !n
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
    {
        return None;
    }
    Some(n.to_string())
}

/// GET /api/vm/isos — the ISOs HomeVault manages (in `{DATA_DIRECTORY}/vm/isos`).
pub async fn handle_vm_isos(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    let dir = match ensure_iso_dir() {
        Ok(d) => d,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let mut list: Vec<IsoEntry> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".iso") {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            list.push(IsoEntry {
                virtio: name.to_ascii_lowercase().contains("virtio"),
                path: e.path().to_string_lossy().to_string(),
                name,
                size,
            });
        }
    }
    list.sort_by_key(|a| a.name.to_lowercase());
    ResponseHelper::json_success(list)
}

/// POST /api/vm/isos/download — fetch an ISO from an https URL into the store.
pub async fn handle_vm_iso_download(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<IsoDownloadRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let url = req.url.trim().to_string();
    if !url.starts_with("https://") || url.len() > 2048 {
        return ResponseHelper::json_error(
            "The download link must be an https:// address.".to_string(),
        );
    }
    let name = req.name.as_deref().and_then(safe_iso_name).or_else(|| {
        url.split('?')
            .next()
            .and_then(|u| u.rsplit('/').next())
            .and_then(safe_iso_name)
    });
    let name = match name {
        Some(n) => n,
        None => {
            return ResponseHelper::json_error(
                "Give the download a name ending in .iso (letters, digits, dashes).".to_string(),
            )
        }
    };
    start_iso_download(&admin, &url, &name)
}

/// POST /api/vm/isos/virtio — one-click VirtIO drivers for Windows.
pub async fn handle_vm_iso_virtio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    start_iso_download(&admin, VIRTIO_WIN_URL, "virtio-win.iso")
}

fn start_iso_download(admin: &str, url: &str, name: &str) -> Response {
    let dir = match ensure_iso_dir() {
        Ok(d) => d,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let dest = dir.join(name);
    if dest.exists() {
        return ResponseHelper::json_error(format!("\"{}\" is already in the store.", name));
    }
    let job = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("vm_iso_download".to_string()),
        &format!("Downloading {}", name),
        Some(admin.to_string()),
    );
    let url = url.to_string();
    let name_owned = name.to_string();
    let name_resp = name.to_string();
    let dest_job = dest.clone();
    tokio::spawn(async move { run_iso_download(&job, &url, &dest_job, &name_owned).await });
    ResponseHelper::json_success(serde_json::json!({ "name": name_resp, "status": "downloading" }))
}

async fn run_iso_download(job: &str, url: &str, dest: &std::path::Path, name: &str) {
    use tokio::io::AsyncWriteExt;

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(6 * 60 * 60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            PROGRESS_MANAGER.fail_job(job, &format!("Download client error: {e}"));
            return;
        }
    };
    let resp = match client.get(url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            PROGRESS_MANAGER.fail_job(job, &format!("The server returned HTTP {}.", r.status()));
            return;
        }
        Err(e) => {
            PROGRESS_MANAGER.fail_job(job, &format!("Could not reach the download link: {e}"));
            return;
        }
    };
    let total = resp.content_length();
    let mut file = match tokio::fs::File::create(dest).await {
        Ok(f) => f,
        Err(e) => {
            PROGRESS_MANAGER.fail_job(job, &format!("Could not write the file: {e}"));
            return;
        }
    };
    let mut stream = resp.bytes_stream();
    let mut done: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(dest).await;
                PROGRESS_MANAGER.fail_job(job, &format!("The download was interrupted: {e}"));
                return;
            }
        };
        if file.write_all(&chunk).await.is_err() {
            let _ = tokio::fs::remove_file(dest).await;
            PROGRESS_MANAGER.fail_job(job, "Could not write to disk (out of space?).");
            return;
        }
        done += chunk.len() as u64;
        if let Some(t) = total.filter(|t| *t > 0) {
            PROGRESS_MANAGER.emit_progress(
                job,
                (done as f64 / t as f64 * 100.0).min(99.0),
                &format!("{} of {}", human_bytes(done), human_bytes(t)),
            );
        } else {
            PROGRESS_MANAGER.emit_progress(job, 0.0, &format!("{} downloaded", human_bytes(done)));
        }
    }
    let _ = file.flush().await;
    drop(file);

    if std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0) < 10_000_000 {
        let _ = std::fs::remove_file(dest);
        PROGRESS_MANAGER.fail_job(job, "That link did not return a valid ISO.");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(dest) {
            let mode = m.permissions().mode();
            let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode | 0o044));
        }
    }
    PROGRESS_MANAGER.complete_job(job, &format!("{} is ready", name));
    crate::events::log_event(format!("VM ISO '{}' downloaded", name));
}

/// DELETE /api/vm/isos/{name}
pub async fn handle_vm_iso_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let name = match safe_iso_name(&name) {
        Some(n) => n,
        None => return ResponseHelper::json_error("Invalid ISO name.".to_string()),
    };
    let dest = match ensure_iso_dir() {
        Ok(d) => d.join(&name),
        Err(e) => return ResponseHelper::json_error(e),
    };
    if !dest.is_file() {
        return ResponseHelper::json_error("That ISO is not in the store.".to_string());
    }
    match std::fs::remove_file(&dest) {
        Ok(()) => {
            crate::events::log_event(format!("VM ISO '{}' deleted by {}", name, admin));
            ResponseHelper::json_success(format!("Deleted \"{}\".", name))
        }
        Err(e) => ResponseHelper::json_error(format!("Could not delete it: {e}")),
    }
}

fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, U[0])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

/// Bridge the browser's xterm to an Incus interactive `exec` shell. Text
/// frames from the browser are `{cols,rows}` resize messages; binary frames
/// are keystrokes. Everything Incus emits is forwarded to the browser as
/// binary.
async fn console_relay(browser: WebSocket, username: String, vm: String, cols: u32, rows: u32) {
    crate::events::log_event(format!("Console opened on VM '{}' by {}", vm, username));

    let session: ExecSession = match IncusService::exec_shell(&vm, cols, rows).await {
        Ok(s) => s,
        Err(e) => {
            let (mut tx, _rx) = browser.split();
            let hint = match e {
                IncusError::Operation(_) => format!(
                    "\r\n\x1b[33mThe machine is not ready for a console yet — it may still be \
                     starting up. Try again in a moment.\x1b[0m\r\n({e})\r\n"
                ),
                _ => format!("\r\n\x1b[31mConsole unavailable: {e}\x1b[0m\r\n"),
            };
            let _ = tx.send(Message::Text(hint.into())).await;
            let _ = tx.close().await;
            return;
        }
    };

    let data_ws =
        match IncusService::open_operation_ws(&session.operation_id, &session.fd_0_secret).await {
            Ok(w) => w,
            Err(e) => {
                let (mut tx, _rx) = browser.split();
                let _ = tx
                    .send(Message::Text(
                        format!("\r\n\x1b[31mConsole connection failed: {e}\x1b[0m\r\n").into(),
                    ))
                    .await;
                let _ = tx.close().await;
                return;
            }
        };
    // Control channel is optional — resize just won't work without it.
    let control_ws = if session.control_secret.is_empty() {
        None
    } else {
        IncusService::open_operation_ws(&session.operation_id, &session.control_secret)
            .await
            .ok()
    };

    let (mut browser_tx, mut browser_rx) = browser.split();
    let (mut incus_tx, mut incus_rx) = data_ws.split();
    let control = control_ws.map(|w| Arc::new(tokio::sync::Mutex::new(w)));

    // Incus -> browser
    let mut to_browser = tokio::spawn(async move {
        while let Some(msg) = incus_rx.next().await {
            match msg {
                Ok(TMessage::Binary(b)) => {
                    if browser_tx.send(Message::Binary(b)).await.is_err() {
                        break;
                    }
                }
                Ok(TMessage::Text(t)) => {
                    if browser_tx
                        .send(Message::Binary(t.as_bytes().to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(TMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        let _ = browser_tx.close().await;
    });

    // browser -> Incus (+ resize on the control channel)
    let control_for_input = control.clone();
    let mut to_incus = tokio::spawn(async move {
        while let Some(Ok(msg)) = browser_rx.next().await {
            match msg {
                Message::Binary(b) => {
                    if incus_tx
                        .send(TMessage::Binary(b.to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Message::Text(t) => {
                    if let (Some(ctrl), Ok(v)) = (
                        control_for_input.as_ref(),
                        serde_json::from_str::<serde_json::Value>(&t),
                    ) {
                        let c = v.get("cols").and_then(|n| n.as_u64()).unwrap_or(0);
                        let r = v.get("rows").and_then(|n| n.as_u64()).unwrap_or(0);
                        if c > 0 && r > 0 {
                            let payload = serde_json::json!({
                                "command": "window-resize",
                                "args": { "width": c.to_string(), "height": r.to_string() }
                            });
                            let _ = ctrl
                                .lock()
                                .await
                                .send(TMessage::Text(payload.to_string().into()))
                                .await;
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = incus_tx.send(TMessage::Close(None)).await;
    });

    tokio::select! {
        _ = &mut to_browser => to_incus.abort(),
        _ = &mut to_incus => to_browser.abort(),
    }

    crate::events::log_event(format!("Console on VM '{}' closed by {}", vm, username));
}

// ── Mapping helpers ─────────────────────────────────────────────────────────

fn instance_to_info(inst: &IncusInstance) -> VmInfo {
    let kind = if inst.instance_type == "virtual-machine" {
        "vm"
    } else {
        "container"
    };
    let status = normalise_status(inst.status_code, &inst.status);
    let running = inst.status_code == STATUS_RUNNING;

    let cpu_cores = inst
        .config
        .get("limits.cpu")
        .and_then(|v| v.as_str())
        .and_then(|s| s.trim().parse::<u64>().ok());

    let memory_mb = inst
        .config
        .get("limits.memory")
        .and_then(|v| v.as_str())
        .and_then(parse_size_to_mib);

    let disk_gb = inst
        .devices
        .get("root")
        .or_else(|| inst.expanded_devices.get("root"))
        .and_then(|d| d.get("size"))
        .and_then(|v| v.as_str())
        .and_then(parse_size_to_mib)
        .map(|mib| (mib as f64 / 1024.0).round() as u64)
        .filter(|v| *v > 0);

    VmInfo {
        name: inst.name.clone(),
        kind: kind.to_string(),
        status: status.to_string(),
        running,
        cpu_cores,
        memory_mb,
        disk_gb,
        ipv4: first_ipv4(inst),
        os: pretty_os(inst),
        created_at: inst.created_at.clone(),
        snapshot_count: inst.snapshots.as_ref().map(|s| s.len()).unwrap_or(0),
        has_screen: vnc_display(inst).is_some(),
    }
}

fn normalise_status(code: u16, raw: &str) -> &'static str {
    match code {
        103 => "running",
        102 => "stopped",
        105 | 106 | 109 | 111 | 200 => "starting",
        104 | 107 | 108 | 110 => "stopping",
        112 => "error",
        _ => match raw.to_lowercase().as_str() {
            "running" => "running",
            "stopped" => "stopped",
            _ => "unknown",
        },
    }
}

/// Parse `"2048MiB"`, `"2GiB"`, `"512MB"`, `"20GiB"` or a plain byte count into
/// MiB. A locale comma is tolerated. Percentages return `None`.
fn parse_size_to_mib(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    if s.ends_with('%') || s.is_empty() {
        return None;
    }
    let (num, unit): (String, String) = s.chars().partition(|c| c.is_ascii_digit() || *c == '.');
    let value: f64 = num.parse().ok()?;
    let mib = match unit.trim().to_lowercase().as_str() {
        "" | "b" => value / (1024.0 * 1024.0),
        "kb" | "kib" | "k" => value / 1024.0,
        "mb" | "mib" | "m" => value,
        "gb" | "gib" | "g" => value * 1024.0,
        "tb" | "tib" | "t" => value * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(mib.round() as u64)
}

fn first_ipv4(inst: &IncusInstance) -> Option<String> {
    let networks = inst.state.as_ref()?.network.as_ref()?;
    for (iface, net) in networks {
        if iface == "lo" {
            continue;
        }
        for addr in &net.addresses {
            if addr.family == "inet" && !addr.address.starts_with("127.") {
                return Some(addr.address.clone());
            }
        }
    }
    None
}

fn pretty_os(inst: &IncusInstance) -> Option<String> {
    let get = |k: &str| {
        inst.config
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    };
    // Prefer "<OS> <release>" — "Ubuntu 24.04", "Debian bookworm" — it reads
    // far better than the raw image description ("… amd64 (20260909_05:24)").
    if let Some(os) = get("image.os") {
        return Some(match get("image.release") {
            Some(rel) => format!("{} {}", capitalise(os), rel),
            None => capitalise(os),
        });
    }
    // Fall back to the description with the build-date suffix trimmed off.
    get("image.description").map(|d| {
        d.split(" (")
            .next()
            .unwrap_or(d)
            .trim_end_matches(" amd64")
            .trim_end_matches(" arm64")
            .trim()
            .to_string()
    })
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ── Validation ──────────────────────────────────────────────────────────────

fn valid_instance_name(name: &str) -> bool {
    let n = name.len();
    if !(1..=63).contains(&n) {
        return false;
    }
    let bytes = name.as_bytes();
    let ok_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
    if !bytes.iter().all(|b| ok_char(*b)) {
        return false;
    }
    let first = bytes[0];
    let last = bytes[n - 1];
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && (last.is_ascii_lowercase() || last.is_ascii_digit())
        && !name.contains("--")
}

fn valid_unix_username(name: &str) -> bool {
    let n = name.len();
    if !(1..=32).contains(&n) {
        return false;
    }
    let bytes = name.as_bytes();
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first == b'_') {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
}

fn valid_snapshot_name(name: &str) -> bool {
    let n = name.len();
    (1..=64).contains(&n)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && !name.starts_with('.')
}

/// Validate an admin-picked local `.iso` path. Admin-only endpoint, so this
/// checks shape and the file itself, not a browse-root allow-list. Returns the
/// canonical path.
fn validate_local_iso(path: &str) -> Result<String, String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err("The ISO path must be an absolute path.".to_string());
    }
    if path.split('/').any(|c| c == "..") || path.contains('\0') {
        return Err("The ISO path is not valid.".to_string());
    }
    if !path.to_ascii_lowercase().ends_with(".iso") {
        return Err("Choose a file that ends in .iso".to_string());
    }
    let canon = std::fs::canonicalize(p).map_err(|_| "That ISO file was not found.".to_string())?;
    let meta = std::fs::metadata(&canon).map_err(|_| "That ISO file was not found.".to_string())?;
    if !meta.is_file() {
        return Err("That path is not a file.".to_string());
    }
    if meta.len() < 10_000_000 {
        return Err("That file is too small to be an installation ISO.".to_string());
    }
    ensure_readable_by_qemu(&canon)?;
    Ok(canon.to_string_lossy().to_string())
}

/// qemu runs unprivileged (`-runas nobody`), so it must be able to *read* the
/// ISO **and traverse every parent directory**. We `chmod` best-effort
/// (HomeVault runs as root in production) and, if something still blocks the
/// path, name the exact offending directory so the person can move the file.
#[cfg(unix)]
fn ensure_readable_by_qemu(canon: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    // The file itself needs o+r.
    if let Ok(meta) = std::fs::metadata(canon) {
        let mode = meta.permissions().mode();
        if mode & 0o004 == 0 {
            let mut perms = meta.permissions();
            perms.set_mode(mode | 0o004);
            let _ = std::fs::set_permissions(canon, perms);
        }
    }
    if std::fs::metadata(canon)
        .map(|m| m.permissions().mode() & 0o004 == 0)
        .unwrap_or(true)
    {
        return Err(format!(
            "The server cannot read that file. Make it readable (chmod +r \"{}\") or move it to a shared folder.",
            canon.display()
        ));
    }

    // Every parent directory needs o+x to be traversable.
    for dir in canon.ancestors().skip(1) {
        if dir.as_os_str().is_empty() {
            break;
        }
        if let Ok(meta) = std::fs::metadata(dir) {
            let mode = meta.permissions().mode();
            if mode & 0o001 == 0 {
                let mut perms = meta.permissions();
                perms.set_mode(mode | 0o001);
                let _ = std::fs::set_permissions(dir, perms);
            }
        }
        let blocked = std::fs::metadata(dir)
            .map(|m| m.permissions().mode() & 0o001 == 0)
            .unwrap_or(false);
        if blocked {
            return Err(format!(
                "The folder \"{}\" is not accessible to the virtual-machine service. Put the ISO in a shared folder instead.",
                dir.display()
            ));
        }
        if dir == std::path::Path::new("/") {
            break;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_readable_by_qemu(_canon: &std::path::Path) -> Result<(), String> {
    Ok(())
}

/// Is Secure Boot actually available on this host? Needs the Microsoft-signed
/// OVMF variables firmware (`ovmf` package). Without it a Windows guest with
/// Secure Boot on will refuse to boot.
fn ovmf_secureboot_available() -> bool {
    [
        "/usr/share/OVMF/OVMF_VARS_4M.ms.fd",
        "/usr/share/OVMF/OVMF_VARS.ms.fd",
        "/usr/share/qemu/OVMF_VARS_4M.ms.fd",
        "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
    ]
    .iter()
    .any(|p| std::path::Path::new(p).exists())
}

/// Can this host give a VM an emulated TPM 2.0 (Windows 11)? Needs
/// `swtpm_setup` from the `swtpm-tools` package — `swtpm` alone is not enough.
fn swtpm_available() -> bool {
    ["/usr/bin/swtpm_setup", "/usr/sbin/swtpm_setup"]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
        || which("swtpm_setup")
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
fn os_or_error(id: &str) -> Result<&'static incus::OsOption, Response> {
    incus::os_catalogue()
        .iter()
        .find(|o| o.id == id)
        .ok_or_else(|| {
            ResponseHelper::json_error("Pick an operating system from the list.".to_string())
        })
}

/// Attaches the virtual sound card when the wizard's "sound" box is ticked.
/// VMs only: containers share the host audio stack, so the device would be
/// rejected there.
fn apply_audio_device(
    devices: &mut HashMap<String, serde_json::Value>,
    audio: bool,
    is_vm: bool,
) {
    if audio && is_vm {
        devices.insert("audio0".to_string(), incus::audio_device());
    }
}

fn incus_err(e: IncusError) -> Response {
    match e {
        IncusError::NotAvailable => ResponseHelper::json_error(
            "Incus is not installed or not running on this server.".to_string(),
        ),
        IncusError::InstanceNotFound(n) => {
            ResponseHelper::json_error(format!("The machine \"{}\" no longer exists.", n))
        }
        other => ResponseHelper::json_error(other.to_string()),
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_audio_device_attaches_for_vms_only() {
        let mut vm_devices = HashMap::new();
        apply_audio_device(&mut vm_devices, true, true);
        assert!(vm_devices.contains_key("audio0"));

        let mut quiet = HashMap::new();
        apply_audio_device(&mut quiet, false, true);
        assert!(!quiet.contains_key("audio0"));

        let mut container = HashMap::new();
        apply_audio_device(&mut container, true, false);
        assert!(!container.contains_key("audio0"));
    }

    #[test]
    fn test_valid_instance_name() {
        assert!(valid_instance_name("web"));
        assert!(valid_instance_name("my-vm-2"));
        assert!(valid_instance_name("a"));
        assert!(!valid_instance_name(""));
        assert!(!valid_instance_name("-web"));
        assert!(!valid_instance_name("web-"));
        assert!(!valid_instance_name("Web")); // uppercase
        assert!(!valid_instance_name("my vm"));
        assert!(!valid_instance_name("my..vm"));
        assert!(!valid_instance_name("a--b")); // double dash
        assert!(!valid_instance_name(&"x".repeat(64)));
    }

    #[test]
    fn test_valid_unix_username() {
        assert!(valid_unix_username("admin"));
        assert!(valid_unix_username("_svc"));
        assert!(valid_unix_username("user-1"));
        assert!(!valid_unix_username(""));
        assert!(!valid_unix_username("1abc")); // leading digit
        assert!(!valid_unix_username("Admin"));
        assert!(!valid_unix_username("has space"));
        assert!(!valid_unix_username(&"a".repeat(33)));
    }

    #[test]
    fn test_valid_snapshot_name() {
        assert!(valid_snapshot_name("before-upgrade"));
        assert!(valid_snapshot_name("2026.01.01_backup"));
        assert!(!valid_snapshot_name(""));
        assert!(!valid_snapshot_name(".hidden"));
        assert!(!valid_snapshot_name("bad/name"));
        assert!(!valid_snapshot_name("bad name"));
    }

    #[test]
    fn test_size_preset() {
        assert_eq!(size_preset("small"), (1, 1024, 10));
        assert_eq!(size_preset("medium"), (2, 2048, 25));
        assert_eq!(size_preset("large"), (4, 8192, 60));
        assert_eq!(size_preset("weird"), (2, 2048, 25));
    }

    #[test]
    fn test_normalise_status() {
        assert_eq!(normalise_status(103, "Running"), "running");
        assert_eq!(normalise_status(102, "Stopped"), "stopped");
        assert_eq!(normalise_status(106, "Starting"), "starting");
        assert_eq!(normalise_status(107, "Stopping"), "stopping");
        assert_eq!(normalise_status(112, "Error"), "error");
        assert_eq!(normalise_status(0, "Running"), "running");
        assert_eq!(normalise_status(0, "Weird"), "unknown");
    }

    #[test]
    fn test_parse_size_to_mib() {
        assert_eq!(parse_size_to_mib("2048MiB"), Some(2048));
        assert_eq!(parse_size_to_mib("2GiB"), Some(2048));
        assert_eq!(parse_size_to_mib("512MB"), Some(512));
        assert_eq!(parse_size_to_mib("20GiB"), Some(20480));
        assert_eq!(parse_size_to_mib("1TiB"), Some(1024 * 1024));
        assert_eq!(parse_size_to_mib("2147483648"), Some(2048)); // raw bytes
        assert_eq!(parse_size_to_mib("50%"), None);
        assert_eq!(parse_size_to_mib(""), None);
        assert_eq!(parse_size_to_mib("1,5GiB"), Some(1536)); // locale comma
    }

    #[test]
    fn test_instance_to_info_maps_the_wire_contract() {
        let inst: IncusInstance = serde_json::from_value(serde_json::json!({
            "name": "web",
            "type": "virtual-machine",
            "status": "Running",
            "status_code": 103,
            "config": {
                "limits.cpu": "4",
                "limits.memory": "8192MiB",
                "image.description": "Ubuntu 24.04 LTS",
                "user.homevault-login": "admin"
            },
            "devices": { "root": { "type": "disk", "path": "/", "size": "40GiB" } },
            "created_at": "2026-02-01T00:00:00Z",
            "state": {
                "status": "Running", "status_code": 103,
                "network": { "eth0": { "addresses": [
                    { "address": "127.0.0.1", "family": "inet", "scope": "local" },
                    { "address": "192.168.1.50", "family": "inet", "scope": "global" }
                ], "hwaddr": "", "state": "up", "type": "broadcast" } }
            },
            "snapshots": [ { "name": "s1", "created_at": "", "stateful": false } ]
        }))
        .unwrap();

        let info = instance_to_info(&inst);
        assert_eq!(info.kind, "vm");
        assert_eq!(info.status, "running");
        assert!(info.running);
        assert_eq!(info.cpu_cores, Some(4));
        assert_eq!(info.memory_mb, Some(8192));
        assert_eq!(info.disk_gb, Some(40));
        assert_eq!(info.ipv4.as_deref(), Some("192.168.1.50"));
        assert_eq!(info.os.as_deref(), Some("Ubuntu 24.04 LTS"));
        assert_eq!(info.snapshot_count, 1);
        assert!(!info.has_screen, "catalogue VM has no VNC screen");
    }

    #[test]
    fn test_vnc_display_and_has_screen() {
        let iso_vm: IncusInstance = serde_json::from_value(serde_json::json!({
            "name": "win11", "type": "virtual-machine", "status": "Running", "status_code": 103,
            "config": {
                "raw.qemu": "-vnc 127.0.0.1:42,share=force-shared",
                "user.homevault-vnc": "42"
            },
            "devices": {}
        }))
        .unwrap();
        assert_eq!(vnc_display(&iso_vm), Some(42));
        assert!(instance_to_info(&iso_vm).has_screen);

        let no_key: IncusInstance = serde_json::from_value(serde_json::json!({
            "name": "x", "type": "virtual-machine", "status": "Stopped", "status_code": 102,
            "config": {}, "devices": {}
        }))
        .unwrap();
        assert_eq!(vnc_display(&no_key), None);
        assert!(!instance_to_info(&no_key).has_screen);
    }

    #[test]
    fn test_validate_local_iso() {
        // shape checks (these fail before touching the filesystem)
        assert!(validate_local_iso("relative/path.iso")
            .unwrap_err()
            .contains("absolute"));
        assert!(validate_local_iso("/srv/iso/../secret.iso")
            .unwrap_err()
            .contains("not valid"));
        assert!(validate_local_iso("/srv/iso/ubuntu.img")
            .unwrap_err()
            .contains(".iso"));
        assert!(validate_local_iso("/nope/does-not-exist.iso")
            .unwrap_err()
            .contains("not found"));

        // a real small temp file: rejected for size, accepted once big enough
        let dir = std::env::temp_dir();
        let small = dir.join("hv-vmtest-small.iso");
        std::fs::write(&small, b"tiny").unwrap();
        assert!(validate_local_iso(small.to_str().unwrap())
            .unwrap_err()
            .contains("too small"));
        let big = dir.join("hv-vmtest-big.iso");
        std::fs::write(&big, vec![0u8; 12_000_000]).unwrap();
        let ok = validate_local_iso(big.to_str().unwrap()).unwrap();
        assert!(ok.ends_with("hv-vmtest-big.iso"));
        let _ = std::fs::remove_file(&small);
        let _ = std::fs::remove_file(&big);
    }

    #[test]
    fn test_alloc_vnc_display_in_range() {
        let d = alloc_vnc_display().expect("a free display on a test box");
        assert!((VNC_DISPLAY_MIN..=VNC_DISPLAY_MAX).contains(&d));
    }

    #[test]
    fn test_safe_iso_name() {
        assert_eq!(
            safe_iso_name("ubuntu-24.04.iso").as_deref(),
            Some("ubuntu-24.04.iso")
        );
        assert_eq!(
            safe_iso_name("virtio-win.iso").as_deref(),
            Some("virtio-win.iso")
        );
        assert!(safe_iso_name("../etc/passwd").is_none());
        assert!(safe_iso_name("a/b.iso").is_none());
        assert!(safe_iso_name("no-extension").is_none());
        assert!(safe_iso_name(".hidden.iso").is_none());
        assert!(safe_iso_name("bad name.iso").is_none()); // space not allowed
        assert!(safe_iso_name(&format!("{}.iso", "x".repeat(200))).is_none());
    }

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5_368_709_120), "5.0 GB");
    }

    #[test]
    fn test_capability_probes_do_not_panic() {
        // Just exercise them — the value depends on the host.
        let _ = ovmf_secureboot_available();
        let _ = swtpm_available();
    }

    #[test]
    fn test_pretty_os_falls_back_to_os_release() {
        let inst: IncusInstance = serde_json::from_value(serde_json::json!({
            "name": "x", "type": "container", "status": "Stopped", "status_code": 102,
            "config": { "image.os": "debian", "image.release": "12" }
        }))
        .unwrap();
        assert_eq!(pretty_os(&inst).as_deref(), Some("Debian 12"));
    }
}
