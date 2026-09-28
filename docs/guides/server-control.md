# Server Control (restart / shutdown)

**Purpose:** Admin-triggered server restart and shutdown.

## Backend

| Endpoint | Auth | Notes |
|----------|------|-------|
| `POST /server/restart` | admin | restart the HomeVault server process |
| `POST /server/shutdown` | admin | shut down the server |
| `GET /server/info` | auth | IP, port, display address |

`handle_server_restart` / `handle_server_shutdown` are in `src/handlers/system.rs`.
`/server/info` is defined inline in `src/web/mod.rs` (`server_info`,
`src/web/mod.rs:360`).

## Frontend

- Triggered from the desktop top bar / Settings power actions.
- `frontend/src/lib/services/api.ts`: `restartServer`, `shutdownServer`,
  `getServerInfo`.

## Invariants

- Both actions require admin. After restart/shutdown the client session ends;
  the UI should redirect to login (or show a "server unavailable" state).

## Related

- [Settings](./settings.md), [API Contract](../API.md)
