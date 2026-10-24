# Server Control (restart / shutdown)

**Purpose:** Admin-triggered server restart and shutdown.

## Backend

| Endpoint | Auth | Notes |
|----------|------|-------|
| `POST /api/server/restart` | admin | restart the HomeVault server process |
| `POST /api/server/shutdown` | admin | shut down the server |
| `GET /api/server/info` | auth | IP, port, display address |

`handle_server_restart` / `handle_server_shutdown` are in
`src/handlers/system.rs`. `/server/info` is defined inline in `src/web/mod.rs`
(`server_info`, `src/web/mod.rs:360`).

Both routes delegate to the same primitive as the Settings power card and the
MCP `system.reboot` tool — `power::host_power_action(PowerAction::Reboot |
Shutdown)` — so the argv (`sudo systemctl reboot|poweroff`), the sudoers grant
(see `src/setup.rs::power_grants`) and the 30 s `run_timed` deadline are
identical everywhere (ROADMAP 24.2). The primitive is synchronous, so it runs
inside `spawn_blocking`, never on the async runtime (ROADMAP X.1).

The action runs **before** the response is written: `systemctl` returns as
soon as the shutdown is scheduled, so a refusal can still be answered with the
host's own reason. A successful call answers
`{ "success": true, "data": { "message": "Server restart initiated", "status":
"restarting" | "shutting_down" } }`; a refused one answers `500` with
`{ "success": false, "message": "Server restart refused: <host stderr>" }`.

`/api/power/{reboot,shutdown,suspend}` remain the host-level family (reboot /
poweroff / **suspend**) and answer `{ "success": true, "data": { "action": … } }`.
They are aliases over the same primitive — same privilege path, different JSON
for a different screen (Settings → Power).

## Frontend

- Triggered from the desktop top bar power menu (the path a home user hits) and
  from Settings → Power.
- `frontend/src/lib/services/api.ts`: `restartServer`, `shutdownServer`,
  `hostPowerAction`, `getServerInfo`.

## Invariants

- Both actions require admin. After restart/shutdown the client session ends;
  the UI must say so and must never present a refusal as a no-op.
- **Desktop flow (ROADMAP 24.1).** The confirmation dialog is modal and its
  confirm button is disabled for the duration of the request (no double
  submit). After the call:
  - accepted → a persistent `role="status"` band on the desktop
    (“Restarting the server… the connection will drop…”), dismissible;
  - refused → a `role="alert"` band carrying the server's reason verbatim,
    the power menu stays usable so the user can retry;
  - the connection dropping before an answer is **not** a refusal: the band
    says the command may still have been accepted, because a box that is
    already going down never flushes its reply.
  When the socket recovers, the usual session handling applies: a dead session
  lands on the login screen, a live one returns to the desktop.
- **Settings → HTTPS (TLS) card.** A refused restart is reported on the card
  (`role="alert"`) — the newly saved certificate is not live until the process
  restarts, so silence there would read as a successful swap.
- The single privilege path is `sudo systemctl <verb>`; the bare `shutdown`
  binary is neither called nor granted. See [Power](./power.md).

## Related

- [Settings](./settings.md), [Power](./power.md), [API Contract](../API.md)
