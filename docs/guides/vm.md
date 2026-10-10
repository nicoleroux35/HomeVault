# Virtual Machines (Incus)

**Purpose:** let a non-technical operator run a full virtual machine or a
system container on their server — create it, log in, snapshot it, delete it —
without knowing anything about Incus, images, cloud-init or networking.

Every endpoint is **admin-only**: Incus talks to a root-owned Unix socket and
hands out disk and network resources.

## Overview

HomeVault drives Incus over its REST API on `/var/lib/incus/unix.socket`
(`src/incus.rs`). No `virsh`, no libvirt, no XML. VMs are KVM/QEMU guests;
containers are Incus system containers.

The wizard has **two modes**:

1. **Ready-made system** — pick from a curated catalogue; boots in minutes with
   a seeded login. The default.
2. **Install from an ISO** — boot a local `.iso` (Linux or Windows) and run the
   OS installer yourself, on a graphical screen. See *ISO install* below.

For the ready-made path the product makes five decisions so the UI stays short:

| Decision | What HomeVault does |
|----------|---------------------|
| Which image | A curated catalogue (`incus::os_catalogue()`) — Ubuntu, Debian, Fedora, Rocky, Alpine — all cloud-init enabled. Resolved against the public `images.linuxcontainers.org` server. |
| How big | Three presets (`small` 1 CPU/1 GB/10 GB, `medium` 2/2/25, `large` 4/8/60). Advanced users can override each number. |
| Networking | The instance inherits the `default` profile's NIC (the bridge `incus admin init` created). Nothing to choose. |
| Secure Boot / TPM | Left at the Incus default — every catalogue image is signed. Not surfaced. |
| How to log in | The wizard asks for a username (default `admin`) + password. `build_cloud_init()` seeds that account with passwordless sudo and SSH password auth. The password is stored **only** as a SHA-512 crypt hash in `cloud-init.user-data`. |

## Backend

REST routes are registered in `src/web/mod.rs` (`/api/vm/*`); the console
WebSocket is a nested router built by `src/handlers/vm.rs::create_vm_router`.

| Endpoint | Method | Notes |
|----------|--------|-------|
| `vm/available` | GET | `{ available: bool }` — is the Incus daemon reachable |
| `vm/os-options` | GET | the curated catalogue — `OsOption[]` (works even if Incus is down) |
| `vm/list` | GET | every instance with live status + IPv4 — `VmInfo[]` (one `recursion=2` call) |
| `vm/create` | POST | `CreateVmRequest` → `{ name, status:"creating" }`; the image pull + first boot run as a tracked `vm_create` job |
| `vm/{name}` | GET | `VmDetail` (adds `login_user`, live memory/disk use, snapshots) |
| `vm/{name}` | DELETE | stops the machine first if it is running, then deletes |
| `vm/{name}/start` | PUT | waits for the operation (up to 40 s) |
| `vm/{name}/stop` | PUT | graceful, then a forced retry if the guest ignores it |
| `vm/{name}/restart` | PUT | graceful, forced retry fallback |
| `vm/{name}/config` | PUT | `{ cpu_cores?, memory_mb? }` — merged with `PATCH`, needs a reboot |
| `vm/{name}/snapshots` | GET/POST | list / create (`{ name, stateful? }`) |
| `vm/{name}/snapshots/{snap}/restore` | POST | stops if running → restores → restarts if it was running |
| `vm/{name}/snapshots/{snap}` | DELETE | |
| `vm/console/ticket` | POST | `{ name, cols?, rows? }` → `{ ticket }` (single-use, 20 s TTL); machine must be running |
| `vm/console/ws` | GET | `?ticket=…` — WebSocket relay to an interactive shell |
| `vm/{name}/screen/ticket` | POST | `{ ticket }` — for an ISO-install VM's graphical screen; machine must be running and have `user.homevault-vnc` |
| `vm/screen/ws` | GET | `?ticket=…` — WebSocket ↔ the VM's loopback VNC (noVNC) |
| `vm/isos` | GET | the ISOs HomeVault manages in `{DATA_DIRECTORY}/vm/isos` — `IsoEntry[]` |
| `vm/isos/download` | POST | `{ url, name? }` — fetch an https `.iso` into the store as a `vm_iso_download` job |
| `vm/isos/virtio` | POST | one-click download of the latest stable `virtio-win.iso` |
| `vm/isos/{name}` | DELETE | remove a managed ISO |

### Request / response shapes

**CreateVmRequest** — `name` (1–63, `[a-z0-9-]`, no leading/trailing/double
dash — the handler also lower-cases it), `size` (`small|medium|large`, default
`medium`), `cpu_cores?` / `memory_mb?` / `disk_gb?` (override the preset,
clamped). Then **exactly one of**:

* *ready-made*: `kind` (`vm` default | `container`), `os` (catalogue id),
  `username` (default `admin`, `[a-z_][a-z0-9_-]*`), `password` (6–128).
* *ISO*: `iso_path` (absolute, no `..`, ends `.iso`, exists, ≥ 10 MB),
  `iso_os` (`linux` | `windows`), `drivers_iso_path?` (Windows VirtIO ISO),
  `secure_boot?` (default: on for Windows, off for Linux). Forces a VM,
  `source.type = none`, no cloud-init.

**VmInfo** — `name`, `kind`, `status`
(`running|stopped|starting|stopping|error|unknown`, normalised from the Incus
status code), `running`, `cpu_cores`, `memory_mb`, `disk_gb`, `ipv4`, `os`
(pretty label), `created_at`, `snapshot_count`, `has_screen` (true for an
ISO-install VM).

**VmDetail** — `VmInfo` flattened + `login_user`, `memory_used_mb`,
`disk_used_gb`, `snapshots: { name, created_at, stateful }[]`.

**OsOption** — `id`, `label`, `kinds` (`["vm","container"]`), `alias`,
`blurb` (one plain-language line), `recommended`.

### Long operations

`create` returns immediately. The image download + first boot (up to 15 min)
run in a spawned task that drives a `PROGRESS_MANAGER` `Custom("vm_create")`
job (visible in the desktop activity taskbar) and, on failure, deletes the
half-built instance so the name frees up. Every other lifecycle call
(`start`/`stop`/`restart`/`delete`/`restore`/snapshot) **waits** for its Incus
operation (`IncusService::wait_for_operation`, `GET /1.0/operations/{id}/wait`)
so the UI never reports a state that has not actually happened.

## ISO install (Linux / Windows)

Picked in the wizard's second mode. The `.iso` is either browsed to on disk or
chosen from the **managed store** (`{DATA_DIRECTORY}/vm/isos`, filled by
`vm/isos/download` — see below). `validate_local_iso()` checks it (absolute, no
traversal, `.iso`, exists, ≥ 10 MB) and `ensure_readable_by_qemu()` makes the
file `o+r` **and every parent directory `o+x`**, so the unprivileged qemu
process can actually reach it — naming the blocking directory if it still
can't. Then `handle_vm_create` builds an **empty** VM (`source.type = none`)
with:

* `raw.qemu = "-vnc 127.0.0.1:5900+N,share=force-shared"` and
  `user.homevault-vnc = N` — a loopback-only VNC server. `alloc_vnc_display()`
  probes `20..=119` for a free port **and** keeps a 60 s reservation so two
  simultaneous creates don't collide.
* `install-media` disk device = the ISO, `boot.priority = 10`;
* an empty `root` disk from the `default` pool;
* **Windows** (`iso_os = windows`): `io.bus = nvme` on the root disk (Setup
  sees it with no extra driver), a `tpm` device **when `swtpm-tools` is
  present**, Secure Boot on **when the host has the Microsoft-signed OVMF**
  (`ovmf_secureboot_available()` — otherwise the create is refused with a
  plain message), bigger size defaults (≥ 2 CPU / 4 GB / 40 GB), and an
  optional `virtio-drivers` CD-ROM.

**TPM self-heal:** an old Incus wires up `swtpm` incorrectly and the VM fails
to start with an opaque error. If a `vtpm` was attached, the `vm_create` job
deletes the failed instance and recreates it **without** the TPM (fresh
display), completing with a note that Windows 11 needs a newer Incus. The
person gets a working, Windows-10-capable VM rather than nothing.

No cloud-init — the person creates their account in the OS installer, on the
graphical screen.

## ISO store (download from a URL / VirtIO drivers)

`{DATA_DIRECTORY}/vm/isos` is a HomeVault-managed folder created `0755` (with
`o+x` on every parent) so qemu can read anything in it without a per-file
chmod. `vm/isos/download` streams an https `.iso` into it as a determinate
`vm_iso_download` `PROGRESS_MANAGER` job (`reqwest` bytes-stream → file, size
sanity-checked, `chmod o+r`); `vm/isos/virtio` is the same with the canonical
`virtio-win.iso` URL baked in. `safe_iso_name()` gates every name (no `/`,
`..`, control chars; must end `.iso`). The wizard shows the store contents as
one-click chips next to the file picker.

## Graphical screen (VNC)

ISO-install VMs get a full framebuffer, driven by **noVNC** (`@novnc/novnc`):

1. `POST /vm/{name}/screen/ticket` — verifies the VM is running and has
   `user.homevault-vnc`; issues a single-use 20 s ticket.
2. Browser opens `wss://…/api/vm/screen/ws?ticket=…` (subprotocol `binary`).
3. `screen_relay()` opens a `TcpStream` to `127.0.0.1:5900+display` and pipes
   raw bytes both ways — RFB in, RFB out. No protocol parsing.

Frontend: `frontend/src/lib/components/vm/VmScreen.svelte` (noVNC `RFB`, a
Ctrl+Alt+Del button, fullscreen).

**Requires `qemu-system-modules-spice`** (`install.sh` installs it). Incus 6.x
always passes `-spice` to qemu, so without that package **no VM boots at all**
(containers are unaffected). `dependencies.rs` reports it — plus `swtpm-tools`
(TPM state init) — under Virtualization.

If the stored VNC port turns out to be taken when a *stopped* screen VM is
started, `lifecycle()` moves it to a free display first (`raw.qemu` +
`user.homevault-vnc` rewritten). Every Incus HTTP call has a 45 s timeout
(`request_json`) so a stuck daemon can't hang the machine list.

## Console

An interactive **shell**, not a VNC framebuffer:

1. Frontend `POST /vm/console/ticket` → single-use token (20 s TTL,
   `RwLock<HashMap>` in `src/handlers/vm.rs`).
2. Frontend opens `wss://…/api/vm/console/ws?ticket=…`; the ticket is consumed
   and admin re-checked at connect time.
3. `IncusService::exec_shell()` runs `POST /1.0/instances/{name}/exec`
   (`interactive`, `wait-for-websocket`) — a root login shell.
4. `IncusService::open_operation_ws()` opens raw WebSockets to the operation's
   data (`fd 0`) and control channels over the Unix socket
   (`tokio-tungstenite`).
5. `console_relay()` bridges: browser binary frames → Incus stdin; Incus
   output → browser binary frames; a browser `{cols,rows}` text frame →
   `window-resize` on the control channel.

For containers this works instantly. For a VM it needs the Incus guest agent,
which comes up a short time after boot — until then the relay returns a clear
"try again in a moment" message instead of a blank screen.

Frontend: `frontend/src/lib/components/vm/VmConsole.svelte` (xterm.js, same
protocol as the Terminal app).

## Security & privacy

- **Admin-only** on every endpoint, re-checked at the console WebSocket
  upgrade.
- Instance / snapshot / username names are validated server-side
  (`valid_instance_name` / `valid_snapshot_name` / `valid_unix_username`) —
  no path traversal, no shell metacharacters, length-bounded — before any
  value reaches an Incus path or the cloud-init YAML.
- The console ticket is single-use with a 20 s TTL — no replay, minimal
  brute-force window.
- Incus errors are re-worded into plain sentences (`incus_err`); raw HTTP
  bodies, socket paths and stack traces never reach the client.
- Console open/close, screen open/close, create/delete, start/stop/restart,
  snapshot and restore are written to the application event log
  (`events::log_event`).
- **ISO path** (`validate_local_iso`): admin-only endpoint, so the check is
  shape + the file (absolute, no `..`, `.iso`, exists, ≥ 10 MB) rather than a
  browse-root allow-list. A best-effort `chmod o+r` lets the unprivileged qemu
  process read it. The VNC server is `127.0.0.1`-only — never reachable off the
  host; the screen ticket is single-use, 20 s.
- **cloud-init password (signed-off exception, 2026-09-09):** the login
  password the operator types in the wizard is transformed to a SHA-512 crypt
  hash (`sha-crypt`) *before* it is placed in `cloud-init.user-data`. The
  plaintext is never logged, never stored, and never sent back. The hash does
  sit in the instance config, which is readable only by root on the host (and
  every VM endpoint is admin-only). This is the minimum needed for a
  non-technical person to actually log into the machine they just made. See
  `PRIVACY.md`.

## Invariants (easy to get wrong)

- `require_admin` on every REST handler **and** re-checked in
  `handle_console_ws` after the ticket is consumed.
- `handle_vm_create` validates everything (name, then either kind/OS/login or
  ISO path/extension/size/free-display) **before** the first Incus call; only
  the "already exists" check and the create itself touch Incus.
- `build_cloud_init` must keep hashing the password — never emit
  `plain_text_passwd`. The unit test asserts the plaintext is absent.
- `os` and `password` are `#[serde(default)]` (optional for ISO mode) — the
  catalogue branch re-checks the password length itself; do not assume a
  missing password is a 422.
- The graphical screen needs `qemu-system-modules-spice` on the host (Incus
  always passes `-spice`; without it **no VM boots**). `raw.qemu -vnc` and
  Incus's `-spice` coexist on the same qemu process — do not remove the
  `-vnc`.
- `wait_for_operation` maps a non-200 operation to `IncusError::Operation`
  carrying `metadata.err` — the lifecycle handlers surface that, they must not
  report success on a failed operation.
- `list_instances` uses `recursion=2` so status, IPv4 and snapshot counts come
  in one round-trip — do not drop back to `recursion=1` (the list would lose
  the IP and the running/stopped truth).
- Incus sends `"field": null` (not absent) for e.g. `state.disk` on a
  freshly-booted VM — the deserialisers use `null_default` for the collection
  fields so `plain #[serde(default)]` cannot blow up the whole list parse.
- `open_operation_ws` connects a fresh `UnixStream` per channel; the console
  relay aborts the partner task as soon as either side closes. `screen_relay`
  does the same over a plain `TcpStream` to `127.0.0.1:5900+display`.
- The `/api/vm` nest (`console/ws`, `screen/ws`) coexists with the
  `/api/vm/{name}` REST routes — matchit allows the static segments alongside
  the `{name}` param. Verified at boot.
- `alloc_vnc_display` bakes a fixed display into `raw.qemu` at create time; a
  restart re-binds the same port. Rare port clashes surface as a start error,
  not silent breakage.

## Frontend

- `frontend/src/routes/VirtualMachines.svelte` — the whole app: one machine
  list, one two-mode create wizard, one detail drawer.
- `frontend/src/lib/components/vm/VmConsole.svelte` — the shell console (xterm).
- `frontend/src/lib/components/vm/VmScreen.svelte` — the graphical screen
  (noVNC, `@novnc/novnc`).
- `frontend/src/lib/components/theme/PathPickerField.svelte` — the ISO / driver
  file pickers (`mode="file"`).
- `frontend/src/lib/services/api.ts` — `getVm*` / `createVm` / `updateVmConfig`
  / `*VmSnapshot` / `getVmConsoleTicket` / `getVmScreenTicket`.
- i18n: `src/i18n/*.toml` `[vm]` (126 keys, 4 locales at parity); `vm.os_*` /
  `vm.status_*` / `vm.w_size_*` are dynamic lookups — see `docs/I18N.md`.

## Requirements

`installer/install.sh` installs
`incus qemu-system-x86 qemu-system-modules-spice swtpm swtpm-tools virtiofsd ovmf`
(best-effort, like Docker) and runs `systemctl enable --now incus`.
`qemu-system-modules-spice` is **not optional for VMs** — Incus 6.x always
passes `-spice` to qemu, so without it every VM fails to start (containers are
fine); `dependencies.rs` flags it. The operator still needs a one-time
`incus admin init --auto` (creates the `default` storage pool + bridge) — a
broken/absent pool makes `create` fail with a plain Incus error, everything
else keeps working.

## Related

- [Docker](./docker.md) · [Storage](./storage.md) · [Terminal](./terminal.md)
  · [Activity feed](./activity.md) · [API Contract](../API.md) ·
  [Backend Structure](../BACKEND.md)
