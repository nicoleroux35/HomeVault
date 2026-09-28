# Downloader (aria2)

**Purpose:** Manage downloads through an aria2 backend. All endpoints are
admin-only (`require_admin_request`).

## Backend

Handlers in `src/handlers/downloader.rs`. The aria2 process is held in
`AppState.downloader` (`Mutex<Aria2Process>`).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /downloader/status` | admin | available/running |
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

## Invariants

- The aria2 daemon is started/stopped via the admin-gated `/downloader/start` and
  `/downloader/stop` endpoints; the process handle lives in `AppState`.
- The desktop activity taskbar reads `aria2.tellActive` **read-only** via
  `GET /api/activity` and only when the daemon is already running — it never
  triggers `ensure_aria2_running`. `src/handlers/downloader.rs` is untouched by
  that feature (the RPC call is re-implemented in `src/handlers/activity.rs`).

## Related

- [API Contract](../API.md), [Activity taskbar](./activity.md)
