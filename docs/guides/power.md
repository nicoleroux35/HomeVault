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
| `GET /power/wol/hosts` | admin | saved machines + the interface evidence (`name`, `address`, `prefix`, `broadcast`, `up`) a packet would use |
| `POST /power/wol/hosts` | admin | add a machine — `{ label, mac, broadcast? }`; the MAC is normalised to `AA:BB:CC:DD:EE:FF`, the label must be unique |
| `PUT /power/wol/hosts/{id}` | admin | edit a machine |
| `DELETE /power/wol/hosts/{id}` | admin | forget a machine |
| `POST /power/wol/wake` | admin | send a magic packet now — `{ id }` for a saved machine or `{ mac }` for a one-off; answers `sent`, never `woken` |

## Wake-on-LAN (QUICK-WIN QW-09)

HomeVault is often installed on a machine that sleeps, and "wake the box, then
run the 03:00 backup" is the natural pairing. `src/wol.rs` (packet, subnets,
destination), `src/handlers/wol.rs` (endpoints),
`src/storage/wol_host_repo.rs` (`wol_hosts`, migration **064**).

**The packet.** 6 × `0xFF` followed by the target MAC sixteen times — 102 bytes,
exactly (`MAGIC_PACKET_LEN`). It is sent to **both** port 9 (`discard`) and port
7 (`echo`): the conventions differ and two 102-byte datagrams cost nothing.

**Where it goes.** A magic packet is a *link-layer* broadcast, so it must leave
the interface that owns the target's segment:

1. the machine's saved `broadcast` override (an IP literal — a relay, another
   VLAN, a test sink), else
2. the **directed broadcast** of the selected interface (`192.168.1.255` for a
   `192.168.1.0/24`), which is the honest default and names the interface in the
   report, else
3. `255.255.255.255` when no interface has an IPv4 address.

The interface is chosen from the inventory the Network screen already reads
(`crate::network::list_interfaces`, no subprocess): with a target address, the
**up** interface whose subnet contains it; without one, the first **up**
interface. A down interface is never chosen, and a non-contiguous netmask is
refused instead of being rounded into a subnet.

**What it says.** The answer is `sent — only confirms it left`, never "woken": a
UDP datagram is unacknowledged and the target is powered off by definition.
That wording is asserted in the unit tests, because "the one case the feature
exists for is the one case it cannot verify".

**Validation.** The MAC accepts `AA:BB:CC:DD:EE:FF`, `aa-bb-…`,
`aabb.ccdd.eeff` and bare hex, and refuses a wrong length, a non-hex character,
`00:00:00:00:00:00` and any **multicast/broadcast** address (`FF:FF:FF:FF:FF:FF`
is every NIC on the segment, never one machine). The override must be an IP
literal: a name would be resolved between a click and a packet. Labels are
trimmed, bounded to 64 characters and unique case-insensitively (checked
explicitly, so the answer is "a host named X already exists" and not a 500 from
the unique index).

**"Wake first" on a scheduled task.** Any task type may carry a pre-step:

```json
{ "type": "command", "command": "/usr/local/bin/backup", "args": [],
  "wake": { "host_id": 2, "delay_seconds": 90 } }
```

`wol::parse_wake_plan` is the single gate: it is validated when the task is
**saved** (a `delay` typo, `host_id: 0` or a delay above 600 s is a 400, never a
task that fires at 03:00 and wakes nothing) and again before the run. The run's
output carries the "sent" sentence and the wait, and a host that was deleted or
cannot be woken **fails the run** — running a backup against a sleeping machine
is exactly what the option exists to prevent. `wake_host_id` is not part of
`TaskAction`: it is a pre-step every task type may carry, not a fifth task type.

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
- **WoL never claims a wake.** The report and the card say the packet left; a
  NIC with Wake-on-LAN disabled in its BIOS or driver ignores it, and nothing in
  the protocol can tell the sender.
- **WoL reads the interface inventory; it never shells out.** `list_interfaces`
  walks `/sys/class/net`, so opening the card or waking a host forks nothing.
- A scheduled task's `wake` pre-step **fails the run** when the host is gone or
  the packet cannot be sent — a silent fall-through would run the task against a
  sleeping machine.

## Frontend

- Settings → System → **Wake-on-LAN card** (`WolCard.svelte`,
  `data-testid="wol-*"`): the interface evidence, the saved machines with
  Wake / Edit / Delete, the add-or-edit form and the "wake first" explanation.
  The wake answer is the server's own sentence, verbatim. The form's row
  heading is a `<span>`, **not** a second `<label for="wol-label">` — two labels
  on one control concatenate into a meaningless accessible name
  ("Add a machine Name") and the E2E count would catch it.
- Settings → System → **Power card**: governor select (disabled when
  unsupported), host action buttons with confirm dialog, spindown table
  (per-disk APM + minutes, add/edit/remove, "Apply now").
- `frontend/src/lib/services/api.ts`: `getPowerStatus`, `setPowerGovernor`,
  `hostPowerAction(action)`, `getPowerSpindown`, `setPowerSpindown`,
  `applyPowerSpindown`, and for WoL `getWolHosts`, `createWolHost`,
  `updateWolHost`, `deleteWolHost`, `wakeWolHost`.
- i18n keys: `power.*` and `settings.wol_*` (4 locales at key parity).

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

Wake-on-LAN:

- Unit (`src/wol.rs`, **17**): the packet's exact shape (6 × `0xFF`, then the
  MAC sixteen times; byte 6 is the MAC, not a seventh `0xFF`), the accepted MAC
  spellings and the refused ones (short/long, non-hex, all-zero,
  multicast/broadcast, with a locally-administered `02:` accepted), the
  netmask↔prefix round trip including a non-contiguous mask, directed broadcast
  (`/24`, `/8`, `/32`, `/0`) and `same_subnet`, `ipv4_interfaces` dropping
  loopback/addressless rows, interface selection by target subnet and by "first
  up", a down interface never chosen, the destination preference
  (override → directed broadcast → limited broadcast) and a named override
  refused, label/destination validation, the report's wording ("only confirms it
  left", never "woken"), and the `wake` pre-step parser (present/absent,
  default delay, `delay` typo, `host_id: 0`, 601 s refused, 600 s accepted).
- Integration (`tests/wol_integration.rs`, **7**): the admin-only matrix on all
  five routes, the CRUD round trip (label trimmed, MAC canonical, duplicate
  refused with a readable reason, second delete a miss), ten invalid hosts
  refused with nothing stored, a **real localhost UDP socket** receiving the 102
  bytes, the wake endpoint reporting `sent` + destination + both ports + the
  "not confirmed" sentence, an ad-hoc MAC, a bad MAC, a missing selector and a
  host that no longer exists, plus the wake-block gate at task-creation time.
- E2E (`tests/e2e/settings/wol.spec.ts`, **13**): card shape, the interface
  evidence or the "no interface" explanation, the empty state's next step,
  add/edit/delete against the API, a bad MAC and a named destination refused
  with the reason on screen, a duplicate name refused, the wake answer's wording
  (`role="status"`, "only confirms it left", never "woken"), a refused wake as
  `role="alert"`, a member refused on read + wake, labels and accessible names
  on the icon-only delete buttons, no duplicate ids, no horizontal overflow.
  `POST /api/power/wol/wake` is **mocked** in this file and `afterAll` sweeps
  every `e2e-wol*` row.

## Related

- [Server control](./server-control.md), [Scheduled tasks](./scheduled-tasks.md),
  [MCP server](./mcp.md), [Storage](./storage.md), [Settings](./settings.md)
