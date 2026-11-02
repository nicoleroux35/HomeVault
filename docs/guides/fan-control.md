# Fan and thermal control

**Purpose:** keep the disks cool without turning the appliance into a jet. The
admin picks a profile (or a fixed speed), HomeVault writes the hwmon PWM
registers, and a watchdog hands control back to the firmware on any error.

**Who can use it:** any signed-in account may read the detected channels and the
active profile. Saving a profile and restoring automatic control are
**admin-only** — writing a PWM value on a machine with no thermal headroom can
cook the disks.

## Where it lives

- **Screen:** Settings → Hardware → **Fan control** →
  `frontend/src/lib/components/settings/FanControlCard.svelte`.
- **Backend:** `src/fan_control.rs` (hwmon discovery, temperature curves,
  hysteresis, PWM writes, laptop detection, failsafe) and
  `src/handlers/fan_control.rs` (HTTP surface).
- **Storage:** `{DATA_DIRECTORY}/data/fan_control.json`.
- **Capability:** `/sys/class/hwmon` directly — no external tool. A host with no
  controllable fan reports zero channels and the card says so.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`  | `/api/system/fans` | `require_auth` |
| `GET`  | `/api/system/fans/config` | `require_auth` |
| `PUT`  | `/api/system/fans/config` | `require_admin` — save **and apply** |
| `POST` | `/api/system/fans/restore-auto` | `require_admin` |

## Rules

- **Hardware first, then disk.** `apply_config` runs before `save_config`, so a
  profile the host refused never becomes the one a restart would re-apply.
- **A laptop is refused, not guessed at.** `is_laptop` short-circuits every
  mutation with a `400` and a sentence explaining that the embedded controller
  owns the fan. A generic Linux device name is never treated as evidence.
- **20 % is a hard floor.** `MIN_SAFE_PWM` (51/255) is enforced inside
  `set_pwm_duty` **and** inside `calculate_duty`, so no curve, no manual value
  and no per-channel override can go below it. The arithmetic is done in `u32`
  because `51 * 100` overflows a `u8` — that overflow used to be a
  compile-time error in the test profile.
- **Hysteresis stops the fans hunting.** A change smaller than the configured
  dead band keeps the previous duty instead of stepping the fan on every tick.
- **Restoring automatic control also resets the saved profile.** Leaving
  "Performance" on disk after handing control back to the firmware would
  re-apply it at the next boot, so the saved profile becomes `Auto` and the
  per-channel overrides are dropped.
- **The guard runs before the body is parsed.** `PUT` takes raw `Bytes` and
  deserializes after `require_admin`.

## Profiles

| Profile | Behaviour |
|---------|-----------|
| `auto` | mode 2 — the firmware drives the fan |
| `quiet` | mode 1 with a slow curve |
| `balanced` | mode 1 with the default curve |
| `performance` | mode 1 with a fast curve |
| `manual` | mode 1 at a fixed duty, floored at 20 % |

Per-channel overrides can pin one channel to a different profile or duty than
the global one; a channel whose override omits a field inherits the global
value.

## Safeguards the card states explicitly

- The speed never drops below 20 %, whatever the curve says.
- On any error the fans go back to firmware control.
- Temperature is read continuously at the configured interval (5–60 s).
- A small temperature change does not move the fans.

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/fan_control.rs` (`#[cfg(test)]`) | curve interpolation, hysteresis, the 20 % floor, profile shape |
| `tests/fan_control_integration.rs` | channel discovery and the config surface |
| `tests/phase4_auth_integration.rs` | `401` without a session, `403` for a member on both mutations, admin never refused |
| `frontend/src/lib/components/settings/FanControlCard.svelte` | covered by `npm run check` |

## Related

- [Power](./power.md) — CPU governor, disk spindown, Wake-on-LAN
- [Health monitoring](./health.md) — temperature and SMART signals
- [Storage](./storage.md) — what the fans are protecting
