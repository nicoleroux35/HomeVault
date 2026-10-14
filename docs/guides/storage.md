# Storage (disks, partitions, virtual disks, snapshots)

**Purpose:** Physical disk/partition inventory, **management of every virtual
disk** (mdadm + ZFS + Btrfs), the disk cache tier, encryption, and storage
health. The [RAID](./raid.md) app only *creates* virtual disks; Storage lists,
verifies, rebuilds and destroys them. **Editable since 2026-09-12** — the
frozen-area registry is gone, so the 2026-09-07 layout redesign is no longer
pinned; the invariants below still bind.

## Backend

Handlers in `src/handlers/storage.rs` (non-RAID parts). Disk enumeration in
`src/disk.rs` (`DiskService::list_partitions` uses `lsblk -l`). Pool
scanning/creation in `src/pools.rs` (`PoolService`); snapshot logic in
`src/snapshots.rs` (`SnapshotService`, consumed by the restic flows — the
standalone `/snapshots/*` HTTP endpoints were removed, see
[Snapshots](./snapshots.md)).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /storage/disks` | auth | physical disks |
| `GET /storage/removable` | auth | removable media |
| `GET /storage/partitions` | auth | `lsblk -l` parse |
| `GET /storage/available-home-disks` | auth | home-folder disk picker (with free/total bytes) |
| `GET /storage/virtual-disks` | auth | **all** virtual disks — mdadm + ZFS + Btrfs (`PoolService::scan_pools`, mdadm `/dev/md/x` normalised to `x`) |
| `DELETE /storage/virtual-disks/{name}` | admin | destroy — dispatches: mdadm → `DiskService::delete_raid`, ZFS/Btrfs → `PoolService::delete_pool` |
| `POST /storage/virtual-disks/{name}/scrub` | admin | verify — ZFS/Btrfs → `PoolService::start_scrub`; mdadm → `echo check > /sys/block/mdX/md/sync_action` |
| `GET /storage/virtual-disks/{name}/scrub` | auth | current scrub / consistency-check status (ZFS `scan:` line, Btrfs `Status:` line, mdadm `sync_action`) + `in_progress` |
| `GET /storage/importable-pools` | auth | "foreign" pools present on attached disks but not imported (ZFS `zpool import`, Btrfs `btrfs filesystem show` minus mounted UUIDs) |
| `POST /storage/import-pool` | admin | import a foreign pool — ZFS `zpool import [-N] <name>` (optional rename), Btrfs mount + `/etc/fstab` persistence |
| `GET /storage/disks/{name}/smart/capabilities` | auth | whether `smartctl` supports this disk |
| `GET /storage/disks/{name}/smart/test` | auth | running SMART self-test status |
| `POST /storage/disks/{name}/smart/test` | admin | start a SMART self-test — `test_type` whitelisted to `short`/`long`/`conveyance` in `DiskService::start_smart_test`; admin-only since it runs a real `smartctl -t` on the host (records an event-log entry). Non-admins see an explanatory message instead of the control |

> The former `GET/POST /storage/pools*` endpoints were **removed** in the
> 2026-09-07 RAID↔Storage re-scope (creation lives in the RAID app,
> management in `virtual-disks` above). `PoolService` is still reached through
> the virtual-disk and cache (`GET /cache/pools`) paths.

Snapshots (separate guide: [Snapshots](./snapshots.md)):
`GET /api/explorer/snapshots?path=`, restic snapshot endpoints, VM snapshot
endpoints.

Disk encryption (separate guide: [Disk Encryption](./encryption.md)):
`GET /encryption/volumes`, `/encryption/available-devices`;
`POST /encryption/volumes`, `/{id}/unlock`, `/{id}/lock`;
`DELETE /encryption/volumes/{id}`.

## RAID ↔ Storage boundary (inverted 2026-09-06)

The split was **reversed**: the RAID app is creation-only, and Storage's
**"Virtual disks"** tab manages every virtual disk. The mechanics:

- `GET|DELETE /api/storage/virtual-disks[/{name}]` + `POST .../{name}/scrub`
  (new handlers in `src/handlers/storage.rs`) are additive and **dispatch by
  type at the handler level**: mdadm → `DiskService::delete_raid` /
  `sync_action`; ZFS/Btrfs → `PoolService`.
- `pools.rs`'s internal `delete_pool` / `start_scrub` **still reject
  `PoolType::Mdadm`** — the new handlers just never call them for mdadm. So
  `pools.rs` and its tests are unchanged; the reversal lives entirely in
  `storage.rs` + `Storage.svelte`.
- The `virtual-disks` handlers never call the mdadm-rejecting `PoolService`
  functions for mdadm — dispatch happens at the handler level. The **Cache
  tier**'s `GET /cache/pools` and `PoolService::scan_pools()` return mdadm
  pools too — designating an existing array as the cache backing
  disk is "use existing storage." See [Disk Cache Tier](./cache.md).

### Live-test fixes (2026-09-07)

Real end-to-end runs on loop devices fixed read-side parsing bugs in
`src/pools.rs::scan_pools`: `get_mdadm_devices` counted the header line;
`get_mdadm_mount_point` matched the symlink instead of `/dev/md127`;
`get_zfs_devices` counted the pool-name / `mirror-0` / `errors:` rows;
mdadm `used_bytes` was a fabricated `size/2`; `get_mdadm_size` read 1K blocks
as bytes; `get_zfs_mount_point` used the wrong `zpool get` property. New
`PoolService::df_usage` helper. Btrfs was already correct.

## Advanced pools

Native ZFS/Btrfs pool creation (with snapshots, checksums, and scrub) lives
in `PoolService` and is reached through the RAID creation flow and the
virtual-disk/cache paths — the former Pools tab and its `/storage/pools*`
endpoints were removed in the 2026-09-07 re-scope. The service mechanics,
still binding:

- `PoolService::create_pool(name, pool_type, raid_profile, devices, mount_point)`
  in `src/pools.rs`. Profiles: ZFS `stripe|mirror|raidz1|raidz2|raidz3`, Btrfs
  `single|raid0|raid1|raid10|raid5|raid6`. ZFS pools self-persist via
  `zpool.cache`; Btrfs pools are mounted at `/mnt/pools/{name}` and persisted
  in `/etc/fstab` under their own `# homevault-pool:{name}` marker (distinct
  from the RAID Manager's `# homevault-raid:{name}` marker — the two
  mount/persist paths are fully independent, by design, so neither can touch
  the other's fstab entries).
- **Btrfs raid5/raid6**: metadata is forced to `raid1` even when data uses
  `raid5`/`raid6`, to avoid the known upstream Btrfs raid5/6 write-hole bug —
  surfaced in the create-pool UI as a tooltip, not silent.
- **Deleting** a Btrfs pool is a real, unrecoverable disk wipe (`wipefs -a` on
  every member device after unmount, since Btrfs has no separate
  "pool"/"filesystem" — the pool *is* the filesystem). The UI requires typing
  the pool name to confirm.
- **Verified live** (loop-device-backed Btrfs raid1 pool, real `mount`/`/etc/fstab`
  entries, confirmed removed byte-for-byte after delete): create → list →
  device listing → snapshot create/list/delete → scrub start/status → pool
  delete, end to end. See Invariants below for two real bugs this surfaced
  and fixed.
- ZFS advanced-pool creation was not live-tested in this pass — no
  `zfsutils-linux` package in the verification environment — but reuses the
  same `zpool create` invocation style already proven by the RAID Manager's
  ZFS pool path in `disk.rs`.

## Frontend

- `frontend/src/routes/Storage.svelte` — 6 tabs: Disks, Partitions,
  **Virtual disks**, Pools, Removable, Encryption. The Virtual-disks tab shows
  kind/profile badges, a health pill, Verify/Rebuild + Destroy per card; the
  Pools tab keeps the native-ZFS/Btrfs create-pool modal + scrub + snapshots.
  Re-skinned to the mono house style 2026-09-07 (E2E hooks `.storage-page`,
  `.storage-stats .storage-stat-card` ×4, `.storage-disk-grid`, 6 `.hv-winhdr-tab`).
- **Storage health (2026-10-17):** each virtual-disk card gained a **Check
  status** button that fetches `GET .../scrub` and renders the scrub state
  inline (`[data-testid="storage-vd-scrub-status"]`). When `zpool import` /
  `btrfs filesystem show` report a pool that isn't imported, a **foreign
  pools** section appears above the list (`[data-testid="storage-importable-pools"]`)
  with an Import button per pool (`POST /storage/import-pool`); ZFS pools can
  be renamed on import. Both are admin-only for the mutating action.
- `frontend/src/lib/services/api.ts`: `getStorageDisks`, `getStoragePartitions`,
  `getStoragePools`, `getStoragePool`, `getStoragePoolDevices`,
  `getStorageRemovable`, `getStorageSystemDisk`, `getScrubStatus`,
  `getAvailableHomeDisks`, `createAdvancedPool`, `deleteAdvancedPool`,
  `startScrub`, `getSnapshotsAvailability`, `getSnapshots`, `createSnapshot`,
  `deleteSnapshot`, `rollbackSnapshot`, `listVirtualDisks`, `deleteVirtualDisk`,
  `scrubVirtualDisk`, plus the encryption methods listed in
  [Disk Encryption](./encryption.md).

## Invariants

- `handle_storage_*` share the real-session-check hardening applied to the RAID
  handlers in the same file — do not revert any handler to a bare "header present"
  check.
- Partition free-space comes from a real `lsblk` read (a past RAID bug hardcoded
  `size/2`).
- `PoolService::parse_size` must keep handling **both** size-suffix styles: the
  single-letter `T`/`G`/`M`/`K` used by `zpool list`/`mdadm`, and the
  two-letter `TiB`/`GiB`/`MiB`/`KiB` used by `btrfs filesystem show`. A real
  bug (found via a live loop-device pool) reported a 600-byte pool for two
  300 MiB devices because only the single-letter suffixes were recognized.
- `scan_zfs_pools`/`scan_mdadm_pools`/`scan_btrfs_pools` must each treat a
  missing binary (`zpool`/`mdadm`/`btrfs` not installed — all three are
  optional packages, see `installer/install.sh`) as "no pools of this type,"
  not a hard error — a real bug had one missing binary abort the *entire*
  pools list for every filesystem type.
- Btrfs snapshot operations need the pool's real **mount point**, not its bare
  name — unlike ZFS, Btrfs has no dataset/pool identifier of its own. A real
  bug had every Btrfs snapshot create/list/delete call fail with "No such file
  or directory" because the bare pool name was passed straight to `btrfs`
  subcommands as if it were a path; fixed via
  `PoolService::btrfs_mount_point` (`pub(crate)`), resolved once at the top
  of each Btrfs-specific function in `src/snapshots.rs`.
- Btrfs snapshot **rollback** of the default (`subvolid=5`) mounted top-level
  subvolume — which is how every pool created here is mounted — has no safe
  single-command equivalent to `zfs rollback`. `rollback_btrfs_snapshot`
  returns a clear, explicit error rather than attempting the naive
  `btrfs subvolume snapshot <snap> <mountpoint>` (which always fails: the
  destination already exists and is mounted) or silently doing nothing.
  ZFS rollback (`zfs rollback -R pool@snapshot`) works correctly in place.
- Every disk-write command in `pools.rs`/`disk.rs`/`encryption.rs`/`acl.rs`
  stays `sudo`-wrapped; `/etc/fstab` is only ever written via
  `write_file_as_root` (`sudo tee`). A 2026-07-17 audit found that on a real
  deployment no pool or encrypted volume survived a reboot because the raw
  `fs::write`/bare `mount` calls silently no-oped for a non-root process.
- `handle_encryption_create` gates `req.device` against
  `DiskService::get_available_devices()` — a past hole let an admin luksFormat
  the boot disk. `encryption::erase` runs a trailing `wipefs -a` so a deleted
  volume's device returns to the available pool.
- **Editable since 2026-09-12** — the frozen-area registry is gone; the
  invariants above stay binding.

## Related

- [RAID](./raid.md), [Snapshots](./snapshots.md), [Explorer](./explorer.md),
  [Disk Encryption](./encryption.md), [Disk Cache Tier](./cache.md)
