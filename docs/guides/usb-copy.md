# USB one-touch copy

**Purpose:** copy a memory card or a USB disk into a folder on the appliance as
soon as it is plugged in, without opening a file manager. Useful for camera
cards, phone backups and any "drop it here" workflow.

**Who can use it:** any signed-in account may read the rules and the attached
devices. Creating, editing, deleting a rule and starting a copy are
**admin-only** — the copy writes into a share as root and mounts the device.

## Where it lives

- **Screen:** desktop app **USB Copy** → `frontend/src/routes/UsbCopy.svelte`
  (Storage category).
- **Backend:** `src/usb_copy.rs` (device discovery, profile validation, copy
  engine, checksum verification, eject) and `src/handlers/usb_copy.rs` (HTTP
  surface).
- **Storage:** `{DATA_DIRECTORY}/data/usb_copy_profiles.json` — a list
  document, so no migration.
- **Dependency:** `udisksctl` from **udisks2** (`src/dependencies.rs`;
  `installer/install.sh` installs it in its required group because the udiskie
  automount unit needs it too). `lsblk` comes from util-linux.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET` | `/api/usb-copy/profiles` | `require_auth` |
| `POST` | `/api/usb-copy/profiles` | `require_admin` |
| `PUT` | `/api/usb-copy/profiles/{id}` | `require_admin` |
| `DELETE` | `/api/usb-copy/profiles/{id}` | `require_admin` |
| `GET` | `/api/usb-copy/devices` | `require_auth` |
| `POST` | `/api/usb-copy/execute/{profile_id}` | `require_admin` |

## Rules

- **The guard runs before the body is parsed.** Mutations take raw `Bytes` and
  deserialize after `require_admin`, so an unauthenticated caller gets `401`
  with the standard envelope and cannot probe the profile schema.
- **The path id wins over the body id.** `PUT` overwrites the body's `id` with
  the one from the URL; a mismatch used to silently move a profile.
- **A copy is scheduled, not awaited.** `POST …/execute` answers as soon as the
  task is spawned, and the task writes `last_run` / `last_status` itself, so an
  outcome is visible even when nothing is polling the response.
- **A disabled rule cannot be run.** Asking to copy with a disabled profile is
  a `400` naming the rule, not a silent no-op.
- **A pool member is refused as a source.** Reading the raw members of an
  mdadm/ZFS/Btrfs array would look like a backup and be useless as one, so
  `is_pool_member` short-circuits the request with a `400`.
- **Every subprocess has a deadline.** `lsblk` runs under `LSBLK_TIMEOUT`, the
  array probes (`mdadm --examine`, `zpool status`, `btrfs filesystem show`)
  under `POOL_PROBE_TIMEOUT`, and mount/unmount/power-off under
  `DEVICE_OP_TIMEOUT`. All go through `utils::process::run_timed_async`, which
  the tree-wide guard enforces.

## The copy itself

1. Resolve the device's mount point; mount it through `udisksctl` when it is
   not already mounted.
2. Copy `source_path` (a folder inside the device) into `destination_path` (an
   absolute path on the appliance).
3. When `verify_checksums` is on, hash every copied file and compare.
4. When `auto_eject` is on, unmount and power the device off — safe to unplug
   immediately.

A profile matches a device on its **UUID**, not its `/dev/sdX` name, which
changes between reboots.

## What the screen shows, and why

- **An empty state that says what to do next** ("No copy rule yet. Create one to
  get started.") rather than an empty table.
- **The attached devices with their UUID**, so the UUID field is a select rather
  than a field the admin has to go and find.
- **Which rule a device matched**, so "I plugged it in and nothing happened" is
  answerable without reading logs.
- **A "not mounted" state** spelled out, never an empty mount point.

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/usb_copy.rs` (`#[cfg(test)]`) | profile validation, destination must be absolute, UUID matching |
| `tests/usb_copy_integration.rs` | device listing and the validation surface |
| `tests/phase4_auth_integration.rs` | `401` without a session, `403` for a member on every mutation, admin never refused |
| `frontend/src/routes/UsbCopy.svelte` | covered by `npm run check` |

## Related

- [File Explorer](./explorer.md) — where the copied files land
- [Storage](./storage.md) — what a "pool member" is, and why it is refused
- [Shares](./shares.md) — sharing the destination folder on the network
