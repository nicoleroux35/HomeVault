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
- `src/storage/disk_power_repo.rs` — `disk_power_settings` (migration
  `045_disk_power.sql`); a row with both fields `None` is deleted.
- `src/handlers/power.rs` — endpoints below; startup re-apply of spindown
  settings in `web/mod.rs::run()` (hdparm's timer is volatile per drive).
- Scheduled power actions go through the
  [Scheduled Tasks](./scheduled-tasks.md) engine (`power` task type).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /power/status` | admin | current governor, kernel-listed governors, capability flags (`governor_supported`, `spindown_supported`) |
| `PUT /power/governor` | admin | `{ "governor": "performance" }` — validated against the kernel list |
| `POST /power/reboot` | admin | host reboot (systemctl) |
| `POST /power/shutdown` | admin | host poweroff |
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

- Unit: power action parse/verbs, governor validation (metacharacters),
  hdparm mapping/args, device-name validation, disk_power repo round trips.
- Integration: `tests/power_integration.rs` (guards, invalid governor /
  device / ranges rejected, status shape).
- E2E: `tests/e2e/settings/power.spec.ts` (live server; read-only
  assertions — never triggers a real power action).

## Related

- [Server control](./server-control.md), [Scheduled tasks](./scheduled-tasks.md),
  [Storage](./storage.md), [Settings](./settings.md)
