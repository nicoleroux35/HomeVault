# Power (host power management + disk spindown)

**Purpose:** Host-level power controls the process-level
[Server control](./server-control.md) never covered: CPU scaling governor,
host reboot / shutdown / suspend, and per-disk spindown (hdparm APM +
idle timer). Inspiration: OMV's `conf.system.powermngmnt` + `hdparm`.

## Backend

- `src/power.rs` — primitives: governor read/set (sysfs through `sudo tee`,
  every cpufreq policy), `PowerAction` (systemctl
  `reboot`/`poweroff`/`suspend`), `DiskSpindownSetting` + hdparm argument
  building (the `-S` minute→code mapping is unit-tested).
  `host_power_command()` / `host_power_argv()` are the **only** place the
  power argv is built: `/api/power/{action}`, `/api/server/*` and the MCP
  `system.reboot` tool all call `host_power_action`, so they share one
  `sudo systemctl <verb>` invocation, one sudoers grant
  (`src/setup.rs::power_grants`) and one 30 s deadline (ROADMAP 24.2). The
  bare `shutdown` binary is not used and not granted.
  Requires the **`hdparm`** package (installed by `installer/install.sh`,
  verified in its Step 4); without it the spindown card reports
  "hdparm not available" and the setting is not applied.
- `src/storage/disk_power_repo.rs` — `disk_power_settings` (migration
  `045_disk_power.sql`); a row with both fields `None` is deleted.
  `delete_all()` drops every row and is what Settings → "Reset to Defaults"
  uses (ROADMAP 13.5) — before that the old `hdparm` values were re-applied on
  the next boot after a reset the UI described as complete.
- `src/handlers/power.rs` — endpoints below; startup re-apply of spindown
  settings in `web/mod.rs::run()` (hdparm's timer is volatile per drive).
  `hdparm --version` (the `spindown_supported` probe) runs in
  `spawn_blocking`, so a status read never forks on the runtime thread
  (ROADMAP X.1).
- Scheduled power actions go through the
  [Scheduled Tasks](./scheduled-tasks.md) engine (`power` task type).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /power/status` | admin | current governor, kernel-listed governors, capability flags (`governor_supported`, `spindown_supported`) |
| `PUT /power/governor` | admin | `{ "governor": "performance" }` — validated against the kernel list |
| `POST /power/reboot` | admin | host reboot (systemctl); alias of `/api/server/restart` over the same primitive |
| `POST /power/shutdown` | admin | host poweroff; alias of `/api/server/shutdown` |
| `POST /power/suspend` | admin | host suspend |
| `GET /power/spindown` | admin | per-disk settings list |
| `PUT /power/spindown` | admin | upsert one disk (`device`, `apm_level` 1-255, `spindown_minutes` 0-330); both `None` clears; applied immediately |
| `POST /power/spindown/apply` | admin | re-apply every saved setting now |

## Invariants

- **Everything is admin-only** — these actions take the whole box down.
- Governor values are validated against `scaling_available_governors` and
  restricted to `[A-Za-z0-9_]` before being written through `sudo tee`.
- Device names are bare kernel names (`sda`, `nvme0n1`) validated by
  `is_valid_block_device_name` before being spliced into `/dev/…` for
  sudo-wrapped hdparm — path traversal / option injection rejected.
- `spindown_minutes` maps to hdparm's coded `-S` values (1-20 min → ×12
  5-second units; 21-330 min → 30-minute blocks 241-251; > 330 rejected,
  never wrapped).
- Applying is best-effort per disk (SSDs / USB bridges may refuse APM):
  failures are logged and surfaced as `applied: false`, the saved setting
  still stands for the next boot.
- Reboot/shutdown kill the process mid-response; the JSON answer is
  best-effort by nature (the UI treats a dead connection as expected).
- `/power/{reboot,shutdown}` and `/server/{restart,shutdown}` are **one
  primitive with two JSON shapes** (ROADMAP 24.2): both run
  `host_power_action` → `sudo systemctl reboot|poweroff`, bounded by
  `run_timed` (30 s) and executed inside `spawn_blocking`. A refused command
  answers `500` with the host's stderr in `message`, never a silent success.
  The sudoers grant names exactly `reboot`, `poweroff` and `suspend` — add a
  verb here and `src/setup.rs::power_grants` follows from `PowerAction`.
- The `hdparm` (spindown) and `systemctl` (power action) calls are bounded
  (ROADMAP X.2): 30 s through `run_timed`, so a wedged controller or a unit
  stuck stopping cannot hold the request forever. The governor write goes
  through `sudo tee`, which streams the value on stdin and is instead kept off
  the runtime (`spawn_blocking`) — it touches only sysfs, with no service to
  wait on.
- Live systemctl/hdparm behaviour is **not** unit-tested (real host actions
  — see CLAUDE.md "What NOT to test in Rust"); integration tests cover
  guards and validation only.

## Frontend

- Settings → System → **Power card**: governor select (disabled when
  unsupported), host action buttons with confirm dialog, spindown table
  (per-disk APM + minutes, add/edit/remove, "Apply now").
- `frontend/src/lib/services/api.ts`: `getPowerStatus`, `setPowerGovernor`,
  `hostPowerAction(action)`, `getPowerSpindown`, `setPowerSpindown`,
  `applyPowerSpindown`.
- i18n keys: `power.*` (4 locales at key parity).

## Tests

- Unit: power action parse/verbs, the single `sudo systemctl` argv for every
  action, governor validation (metacharacters), hdparm mapping/args,
  device-name validation, disk_power repo round trips, and the response
  mapper the two `/api/server/*` handlers use (accepted → `status`, refused
  → `message` with the host's reason).
- Integration: `tests/power_integration.rs` (guards on both families, invalid
  session on every power route, POST-only registration, invalid governor /
  device / ranges rejected, status shape, refusal/accepted bodies). **No test
  POSTs as an admin to a power route**: that would take the host down.
- E2E: `tests/e2e/settings/power.spec.ts` (live server; read-only
  assertions — never triggers a real power action) and
  `tests/e2e/desktop/power.spec.ts` (the desktop flow with
  `POST /api/server/*` mocked: alert on refusal, status on success, no double
  submit, 44px dismiss, phone width).

## Related

- [Server control](./server-control.md), [Scheduled tasks](./scheduled-tasks.md),
  [MCP server](./mcp.md), [Storage](./storage.md), [Settings](./settings.md)
