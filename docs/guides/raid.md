# RAID — virtual-disk creation wizard

**Purpose:** A **creation-only** wizard that turns physical disks into one
**"virtual disk"**. Everything else — listing, verifying/scrubbing, rebuilding,
destroying — lives in the [Storage](./storage.md) app's "Virtual disks" tab.

> The RAID/Storage split was **inverted** on 2026-09-06. The words
> "pool"/"grappe"/"array" are out of the RAID UI/i18n; RAID never lists or
> destroys anything.

## Backend

`handle_raid_create` + `handle_raid_available_devices` in
`src/handlers/storage.rs`. `handle_raid_create` dispatches by target filesystem:

- `btrfs` → native `PoolService::create_pool(name, Btrfs, level, …)` (the `raidN`
  level is the Btrfs profile 1:1).
- `ext4` / `xfs` → mdadm via `DiskService::create_raid` (`src/disk.rs`), then
  `mount_and_persist_raid` writes `/etc/fstab` + `/etc/mdadm/mdadm.conf` so the
  array reassembles after reboot.
- `zfs` → `create_zfs_pool_from_devices` (arg order: `zpool create -f <name>
  [vdev-type] <devices…>` — the name comes **before** the vdev type).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /storage/raid/devices` | auth | candidate physical disks |
| `POST /storage/raid` | admin | create one virtual disk (level → disks → filesystem+name) |

The old `GET /storage/raid` (list), `DELETE /storage/raid/{name}`, and
`POST /storage/raid/{name}/rebuild` routes still exist — Storage's "Virtual
disks" tab reuses the rebuild route; listing/destroying is done through
`GET|DELETE /api/storage/virtual-disks` (see [Storage](./storage.md)).

## Frontend

- `frontend/src/routes/Raid.svelte` — a 3-step wizard rendered inline (Type ·
  Disks · Confirm), re-skinned to the mono house style 2026-09-07. E2E hooks:
  `.raid-root`, `.rl-wizard-card`, `.rl-type-grid`, `.rl-type-card`,
  `.rl-progress-track > span`, `.rl-disk-row.selectable`, `.rl-fs-card`,
  `#raidName`.
- `frontend/src/lib/services/api.ts`: `getRaidAvailableDevices`, `createRaid`.

## Invariants

- Every RAID endpoint requires a **valid session** (a past bug only checked the
  `X-Session-Token` header was non-empty).
- Every disk-touching command in `disk.rs` / `pools.rs` stays `sudo`-wrapped.
- `zpool create`'s pool name precedes the vdev type; the mdadm member-device
  parse reads the detail table's last token, not `parts.first()`.
- No "pool"/"grappe" wording in RAID's UI or i18n; RAID never lists or destroys.
- Real end-to-end run on loop devices (2026-09-07) fixed 4 bugs in `src/disk.rs`
  (`mount`/`umount`/`blkid` were not `sudo`-wrapped; `zpool create` arg order).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Raid.svelte`, the RAID api.ts methods, the RAID handlers in `storage.rs` and
  the `raid.*` i18n section are open to edits. The invariants above stay
  binding: a change keeps the Rust integration + Playwright E2E suites green.

## Related

- [Storage](./storage.md), [Snapshots](./snapshots.md), [Rescue](./rescue.md),
  [API Contract](../API.md)
