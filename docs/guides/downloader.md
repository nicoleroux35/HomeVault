# Downloader (aria2)

**Purpose:** Manage downloads through an aria2 backend. All endpoints are
admin-only (`require_admin_request`).

## Backend

Handlers in `src/handlers/downloader.rs`. The aria2 process is held in
`AppState.downloader` (`Mutex<Aria2Process>`), together with the RPC secret it
was started with and the canonical download root.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /downloader/status` | admin | `{ available, running, dir }` |
| `GET /downloader/global-stat` | admin | global counters |
| `GET /downloader/tasks` | admin | task list |
| `GET /downloader/task/{gid}` | admin | single task |
| `POST /downloader/start\|stop` | admin | daemon control |
| `POST /downloader/add` | admin | `{ uri, dir?, out? }` |
| `POST /downloader/pause\|unpause\|remove` | admin | `{ gid }` |

## Frontend

- `frontend/src/routes/Downloader.svelte`.
- `frontend/src/lib/services/api.ts`: `getDownloaderStatus`, `startDownloader`,
  `stopDownloader`, `getDownloaderGlobalStat`, `getDownloaderTasks`,
  `addDownloaderUrl`, `pauseDownloader`, `unpauseDownloader`, `removeDownloader`,
  `getDownloaderTask`.

## Input confinement

Every field of `POST /downloader/add` is validated **before**
`ensure_aria2_running` is called, so a refused request can never spawn the
daemon as a side effect.

| Field | Rule | Refused examples |
|-------|------|------------------|
| `uri` | Scheme allowlist: `http`, `https`, `ftp`, `sftp`, `magnet`. Plain links need a host; magnet links need `xt=`. No spaces or control characters, ≤ 2048 chars. | `file:///etc/shadow`, `data:…`, `javascript:…`, `smb://…`, `http://` |
| `dir` | **Relative** sub-folder of the download root. No leading `/`, no `..` or `.` segment, no backslash, no control characters, ≤ 128 chars. | `/etc`, `../etc`, `films/../../etc`, `a\b` |
| `out` | Bare file name — never a path. No separator, not `.`/`..`, no control characters, ≤ 255 chars. | `../x`, `a/b`, `/etc/passwd` |

`dir` is always sent to aria2 explicitly (never omitted), so the daemon cannot
fall back to its own working directory.

### Download root

`paths::downloader_dir()` = `<DATA_DIRECTORY>/downloads` (default
`/var/lib/homevault/downloads`). It replaced the old `/tmp/aria2-downloads`,
which was wiped on every reboot — a NAS appliance lost its downloads on
restart. The root is created and canonicalised on start, passed to aria2 as
`--dir`, and `resolve_download_dir` re-checks the **deepest existing ancestor**
of the requested sub-folder against it, so a symlink planted inside the root
cannot redirect a download outside it.

## RPC security and liveness

- **Per-process secret.** Each start generates a fresh `rpc-secret`
  (`hv_` + 128 bits) and writes it to `<DATA_DIRECTORY>/aria2-rpc.conf` with
  mode 0600. aria2 reads it through `--conf-path`; it is **never** passed as
  `--rpc-secret` on the command line, because `/proc/<pid>/cmdline` is
  world-readable (CLAUDE.md, "Secrets never hit a command line"). Every RPC
  call prepends `token:<secret>`. The secret never appears in a response body,
  a log line or the event log — `GET /downloader/status` reports only
  `available`, `running` and `dir`.
- **Timeout.** Each RPC round-trip is bounded (`ARIA2_RPC_TIMEOUT`, 10 s); a
  wedged daemon surfaces an error instead of hanging the handler forever.
- **Health poll.** `ensure_aria2_running` no longer sleeps 500 ms and hopes.
  `wait_until_ready` polls `aria2.getVersion` (10 s deadline, 200 ms probe) and
  kills the child if it exits or never answers, so a successful start means the
  RPC port really is listening. The process lock is held across the bounded
  poll so two concurrent requests cannot spawn two daemons on the same port.
- **Reaping.** `aria2_is_running` and `ensure_aria2_running` call `try_wait`;
  a crashed child is cleared and reported as `running: false` instead of
  reading as alive forever.
- **Shutdown.** `shutdown_signal` in `src/web/mod.rs` takes the downloader
  state and kills + waits the child on Ctrl+C or a systemd stop, so aria2 never
  outlives the server that owns it.

## Invariants

- The daemon is started/stopped only through the admin-gated
  `/downloader/start` and `/downloader/stop` endpoints; the process handle
  lives in `AppState`.
- **The UI never starts the daemon on its own.** Opening the app shows a
  "Start the download service" panel when the daemon is stopped; spawning a
  background process is the user's decision.
- The desktop activity taskbar reads active downloads **read-only** through
  `aria2_active_tasks` (`src/handlers/downloader.rs`, shared with this module
  and secret-aware) via `GET /api/activity`, and only when the daemon is
  already running — it never triggers `ensure_aria2_running`.
- Failures are surfaced: `add`, `pause`, `unpause`, `remove` and `start`
  render the server's reason in a dismissible alert
  (`data-testid="downloader-error"`); no action fails silently.
- A task card is a container, not a button: its selectable part is a real
  `<button data-testid="downloader-card-select">`, and the action buttons are
  siblings — no interactive control ever nests another.

## Residual risk

The allowlist stops a *local* read (`file://`) and the obvious injection
schemes, but a download manager by nature fetches a URL from the server's
network position. An admin can therefore still point aria2 at an internal HTTP
service (SSRF). That is inherent to the feature and bounded by the admin-only
gate; the root confinement is what keeps the *write* side safe.

## Related

- [API Contract](../API.md), [Activity taskbar](./activity.md)
