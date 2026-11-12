# USB one-touch copy

**Purpose:** copy a memory card or a USB disk into a folder on the appliance as
soon as it is plugged in, without opening a file manager. Useful for camera
cards, phone backups and any "drop it here" workflow.

**Who can use it:** any signed-in account may read the rules and the attached
devices. Creating, editing, deleting a rule and starting a copy are
**admin-only** — the copy writes into a share as root and mounts the device.

## Where it lives

- **Screen:** desktop app **USB Copy** → `frontend/src/routes/UsbCopy.svelte`
  (Storage category). It uses the standard v2 window shell — `PageLayout`
  (title + icon + window controls), page actions in the `WindowToolbar`, and
  `SectionCard` for the rules/devices panels — like every other app window.
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
2. Copy `source_path` — a folder **or a single file** inside the device — into
   `destination_path` (an absolute path on the appliance). A single file keeps
   its own name under the destination folder.
3. When `verify_checksums` is on, read every copied file back and compare a
   **SHA-256** of source and destination (same size is not enough), collecting
   every mismatch into one error. A mismatch fails the copy; the caller never
   sees `Success`. Photo import verifies each file as it is written, because
   the destination layout no longer mirrors the source.
4. When `auto_eject` is on, unmount and power the device off — safe to unplug
   immediately.

## Copy modes and conflict handling (FIAB-13)

Each profile carries a **copy mode** (`strategy`) and an **existing-file
policy** (`conflict`), mirroring DSM USB Copy. Both default to the historical
behaviour, so a profile written before FIAB-13 still loads and copies the way
it used to.

| `strategy` | What it does |
|---|---|
| `incremental` (default) | Copies a file when the destination is missing, has a different size, or is older than the source. Unchanged files are left alone, so a nightly card copy only transfers what changed. |
| `mirror` | Incremental, then removes destination files the source no longer has (a true replica). `.versions/` is never touched. |
| `versioned` | Before a file is overwritten, the previous destination copy is moved to `.versions/<YYYYMMDD-HHMMSS>/` next to it (one folder per run), so "I overwrote the wrong card" is recoverable. |
| `photo_import` | Sorts every file into `YYYY/MM/` by its modification date, ignoring the camera's folder names (`DCIM/100CANON`). |
| `conflict` | `overwrite` (default) replaces the file; `skip` keeps the copy already on the NAS; `rename` keeps both as `photo (1).jpg`. |

`mirror` is the only mode that deletes; the screen names the consequence on the
mode itself. The conflict policy is evaluated after the strategy: an up-to-date
file under `incremental` is still skipped rather than duplicated, and `skip` or
`rename` takes precedence over `versioned` (the old copy stays in place or is
renamed, it is not hidden in `.versions`).

### Version retention (FIAB-26)

Every versioned profile also carries `versions_to_keep` (default **10**,
maximum 1 000). After a successful run, the oldest `.versions/<stamp>/` folders
are removed until only that many remain in each folder. The rules that keep
this safe:

- only directories whose name is a `YYYYMMDD-HHMMSS` stamp are candidates — a
  loose file, a `manual/` folder or a user's own timestamp-shaped folder is
  left alone;
- only `.versions` folders inside the destination are visited, and symlinks are
  never followed, so the pass cannot reach outside the destination;
- the walk is bounded (10 000 directories), and **0 disables the cleanup** for
  a profile that wants every version kept;
- cleanup is best-effort: a failed removal never turns a verified copy into a
  failed run.

The value is validated by the same profile gate as the rest of the rule, so
`1001` is a `400` naming the field rather than a silently clamped setting.

## Traceability

Every run writes **one** `transfer_history` row (`operation = usb_copy`) with
the source path, the destination path, the bytes copied, the strategy slug in
`method`, and whether the copy was checksum-verified. It shows up in the File
Explorer's transfer history as "USB copy", so "what did the card copy do last
night?" is answerable without reading the journal. One row per run, not per
file: a photo import would otherwise flood the table.

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
| `src/usb_copy.rs` (`#[cfg(test)]`) | profile validation, destination must be absolute, UUID matching, SHA-256 verification (identical tree, same-size different bytes, size mismatch, missing file, single-file source), file counting, incremental skip of an up-to-date file, mirror deletion of a stale file, versioned move into `.versions/<stamp>/`, photo-import `YYYY/MM` layout, version retention (newest N kept, 0 = keep all, lookalike folders ignored, nested `.versions` walked, stamp shape, retention bound) |
| `tests/usb_copy_integration.rs` | device listing, the validation surface, legacy profile JSON loading with the FIAB-13/FIAB-26 defaults, snake_case literals for every strategy/conflict, retention bounds |
| `tests/phase4_auth_integration.rs` | `401` without a session, `403` for a member on every mutation, admin never refused |
| `frontend/src/routes/UsbCopy.svelte` | covered by `npm run check` |

## Related

- [File Explorer](./explorer.md) — where the copied files land
- [Storage](./storage.md) — what a "pool member" is, and why it is refused
- [Shares](./shares.md) — sharing the destination folder on the network
