# Storage (disks, partitions, virtual disks, snapshots)

**Purpose:** Physical disk/partition inventory, **management of every virtual
disk** (mdadm + ZFS + Btrfs), the disk cache tier, encryption, and storage
health. The [RAID](./raid.md) app only *creates* virtual disks; Storage lists,
verifies, rebuilds and destroys them. **Editable since 2026-09-12** — the
frozen-area registry is gone, so the 2026-09-07 layout redesign is no longer
pinned; the invariants below still bind.

## Backend

Handlers in `src/handlers/storage.rs` (non-RAID parts). Disk enumeration in
`src/disk.rs` (`DiskService::list_partitions` and
`list_removable_disks` both parse `lsblk -P` key/value pairs — never a
whitespace-split table). Pool
scanning/creation in `src/pools.rs` (`PoolService`); snapshot logic in
`src/snapshots.rs` (`SnapshotService`, consumed by the restic flows — the
standalone `/snapshots/*` HTTP endpoints were removed, see
[Snapshots](./snapshots.md)).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /storage/disks` | auth | physical disks |
| `GET /storage/removable` | auth | removable media |
| `GET /storage/partitions` | auth | `lsblk -P` parse |
| `GET /storage/available-home-disks` | auth | home-folder disk picker (with free/total bytes) |
| `GET /storage/virtual-disks` | auth | **all** virtual disks — mdadm + ZFS + Btrfs (`PoolService::scan_pools`, mdadm `/dev/md/x` normalised to `x`) |
| `DELETE /storage/virtual-disks/{name}` | admin | destroy — dispatches: mdadm → `DiskService::delete_raid`, ZFS/Btrfs → `PoolService::delete_pool`. **Refuses to go on when unmount fails** (see "Destroying is unmount-gated" below) |
| `POST /storage/virtual-disks/{name}/scrub` | admin | verify — ZFS/Btrfs → `PoolService::start_scrub`; mdadm → `sudo mdadm --action check <array>` |
| `POST /storage/virtual-disks/{name}/grow` | admin | mdadm only: `{ device }` (a free whole disk) → `DiskService::grow_raid` (`mdadm --add` then `--grow --raid-devices N+1`); raid0/raid10 and ZFS/Btrfs refused; the reshape runs in the background. UI: the "Add disk" modal in Storage's Virtual-disks tab (see [RAID](./raid.md)) |
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
`POST /encryption/volumes` (answers `{ job_id }` — creation is a tracked job),
`/{id}/unlock`, `/{id}/lock`, `/{id}/passphrase`;
`DELETE /encryption/volumes/{id}`.

## RAID ↔ Storage boundary (inverted 2026-09-06)

The split was **reversed**: the RAID app is creation-only, and Storage's
**"Virtual disks"** tab manages every virtual disk. The mechanics:

- `GET|DELETE /api/storage/virtual-disks[/{name}]` + `POST .../{name}/scrub`
  (new handlers in `src/handlers/storage.rs`) are additive and **dispatch by
  type at the handler level**: mdadm → `DiskService::delete_raid` /
  `mdadm --action check`; ZFS/Btrfs → `PoolService`.
- `DELETE /api/storage/raid/{name}` is **gone** (2026-10-23). It was a second,
  untyped destroy path left over from the pre-inversion layout: nothing in the
  UI called it, it bypassed the Storage flow, and it contradicted
  `docs/guides/raid.md` ("RAID never lists or destroys anything"). Destroying
  a virtual disk has exactly one entry point, `DELETE
  /api/storage/virtual-disks/{name}`, which resolves the technology first.
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
  "pool"/"filesystem" — the pool *is* the filesystem). Destruction goes
  through the unified Storage "Virtual disks" card, which requires typing the
  disk name to confirm (see Invariants).
- **Verified live** (loop-device-backed Btrfs raid1 pool, real `mount`/`/etc/fstab`
  entries, confirmed removed byte-for-byte after delete): create → list →
  device listing → snapshot create/list/delete → scrub start/status → pool
  delete, end to end. See Invariants below for two real bugs this surfaced
  and fixed.
- ZFS advanced-pool creation was not live-tested in this pass — no
  `zfsutils-linux` package in the verification environment — but reuses the
  same `zpool create` invocation style already proven by the RAID Manager's
  ZFS pool path in `disk.rs`.

## Deadlines on every host tool (2026-10-24, ROADMAP X.2)

`PoolService` (`src/pools.rs`), `SnapshotService` (`src/snapshots.rs`) and the
scan side of `DiskService` (`src/disk.rs`) shell out to `zpool`, `btrfs`,
`mdadm`, `smartctl`, `df`, `blkid`, `mount`/`umount`, `wipefs` and `mkfs.*`.
Every one of those calls used to be a bare `.output()` — a wedged pool command
held the `spawn_blocking` thread behind it, and therefore the request or the
snapshot job, for the life of the process. They all go through
`crate::utils::process` now (the one reference implementation):

| Module | Reads | Writes |
|---|---|---|
| `pools.rs` | `POOL_QUERY_TIMEOUT` 30 s (`zpool list/status/get`, `btrfs filesystem show/df`, `df`, `blkid`) | `POOL_WRITE_TIMEOUT` 900 s (`zpool create/destroy/import`, `mkfs.btrfs`, `mount`, `umount`, `wipefs`, `rmdir`) |
| `snapshots.rs` | `SNAPSHOT_QUERY_TIMEOUT` 30 s (`zfs list/get`, `btrfs subvolume list`) | `SNAPSHOT_WRITE_TIMEOUT` 600 s (`zfs snapshot/destroy/rollback -R`, `btrfs subvolume snapshot/delete`, `sudo mkdir -p`) |
| `disk.rs` | `BLKID_TIMEOUT` 30 s (`lsblk`, `df`, `blkid`), `SMART_TIMEOUT` 20 s (`smartctl`) | `MDADM_TIMEOUT` 120 s, `MKFS_TIMEOUT` 900 s, `MOUNT_TIMEOUT` 60 s, `ZPOOL_TIMEOUT` 120 s, `INITRAMFS_TIMEOUT` 300 s |

The values are **ceilings, not estimates**: a multi-terabyte `mkfs` or a
recursive `zfs rollback -R` legitimately takes minutes, so a tight probe budget
would break the honest path. What the deadline buys is that a *wedged* tool
becomes a visible, retryable failure instead of a silent stall — and the
message says which: `zpool did not finish within 900 seconds`.

Two shapes worth knowing:

- **`sudo tee` keeps its stdin.** Persisting a mount point means writing
  `/etc/fstab` as root, which is `sudo tee <path>` with the content on stdin.
  `run_timed` closes stdin by design, so that path uses
  `run_timed_with_stdin` (blocking) / `run_timed_async_with_stdin` (async) —
  otherwise `tee` would write an **empty** file and truncate the target. The
  input is written from its own thread, so a child that never reads it cannot
  deadlock the caller before the deadline loop starts.
- **"Not installed" is not "too slow".** The async helper reports a named
  `ProcessFailure::{Spawn, Timeout}`, and `disk.rs` maps them to
  `DiskError::Io` and `DiskError::Timeout` respectively, so the wizard can
  offer the dependency check or a retry — not one merged "failed".

`test_no_subprocess_of_this_module_runs_without_a_deadline` in each of the three
modules fails if a bare `.output()` / `.spawn()` / `.status()` /
`wait_with_output()` ever comes back into production code: a behavioural test
cannot see this defect, because an unbounded call still answers correctly on an
idle box.

## Frontend

- `frontend/src/routes/Storage.svelte` — 6 tabs: Disks, Partitions,
  **Virtual disks**, Pools, Removable, Encryption. The Virtual-disks tab shows
  kind/profile badges, a health pill, Verify/Rebuild + Destroy per card; the
  Pools tab keeps the native-ZFS/Btrfs create-pool modal + scrub + snapshots.
  **Destroy** is a typed confirmation (the disk name has to be typed) and the
  backend refuses to destroy an array it could not unmount.
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

- **A refused action is never silent (ROADMAP X.3, 2026-10-24).** `dlg.notify`
  carries the server's own reason for every failed mutation, including the
  SMART self-test start (which used to have a `// silent error` branch and a
  silent `catch`). Never reintroduce one.
- `handle_storage_*` share the real-session-check hardening applied to the RAID
  handlers in the same file — do not revert any handler to a bare "header present"
  check.
- Partition free-space comes from a real `lsblk` read (a past RAID bug hardcoded
  `size/2`).
- Physical disk enumeration (`DiskService::list_disks`,
  `get_available_devices`, Dashboard/Health/Rescue `lsblk` scans) excludes
  in-memory and flash-translation pseudo-disks (`zram*`, `ram*`, `mtdblock*`)
  via `DiskService::is_pseudo_disk_name`, and reads `/sys/block/mmcblk*/device/{type,name}`
  when `lsblk` leaves `VENDOR`/`MODEL` empty on SD/eMMC cards.
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
- **The sudoers grants are scoped (2026-10-23).** The Storage/LUKS block in
  `SetupService::configure_sudoers` used to grant `zpool`, `zfs`, `mkfs.btrfs`/
  `ext4`/`xfs`, `mount`, `umount`, `wipefs`, `btrfs`, `cryptsetup`, `blkid`,
  `mkdir`, `rmdir` and `tee` **with no argument restriction** — any
  attacker-controlled string reaching an argv became `mkfs.ext4 /dev/sda`,
  `zfs destroy -r tank` or `tee /etc/shadow` as root. The block is now
  rendered from one table (`StorageBins` → `pool_grants` /
  `filesystem_grants` / `mount_grants` / `config_grants`), every entry
  carrying its scope:
  - per subcommand: `zpool create|destroy|import|list|scrub|status`,
    `zfs destroy|get|list|rollback|set|snapshot|userspace`,
    `btrfs filesystem|scrub|subvolume`, `cryptsetup close|erase|
    luksChangeKey|luksFormat|luksUUID|open|status`;
  - per flag shape: `mkfs.ext4 -F …`, `mkfs.xfs -f …`, `mkfs.btrfs -f …`,
    `mkfs.exfat [-n <label>] /dev/…`, `wipefs -a …`,
    `blkid -s UUID -o value …`;
  - per path: `mount [-o <opts>] … /mnt/*`, `umount /mnt/*`,
    `mkdir -p /mnt/*`, `rmdir /mnt/*`, `tee /etc/fstab`,
    `tee /etc/mdadm/mdadm.conf`.
  The SFTP jail root (`/var/lib/homevault-sftp/…`) and removable media
  (`/media`, `/run/media`) keep the `mount`/`umount`/`mkdir`/`rmdir` grants
  they were relying on implicitly through that block, and the other modules
  that had been covered by its `tee` — `/etc/resolv.conf`, the
  netplan/ifupdown file, avahi's config + service (plus the `rm -f` that
  unpublishes it), Docker's `daemon.json`, the cpufreq governor and
  `/proc/sys/net/ipv6/conf/*/*` — now have their own exact-path grant, so
  nothing broke. `mkfs.exfat` (Explorer's "format as
  exFAT") was already a tracked dependency but had **no** grant at all; it is
  covered now. What sudoers cannot constrain is stated in place: the pool
  name and the options after a `zpool`/`zfs` subcommand stay variable, because
  they are the feature. `src/setup.rs`'s tests pin the rendered table (no
  grant may be a bare binary or reach a shell), and the call sites that never
  matched a real privileged shape were fixed in the same pass (next bullet,
  ROADMAP 7.4).
- **The residual call sites are fixed (2026-10-23, ROADMAP 7.4).** Four
  helpers (`mount_disk`, `unmount_disk` and their removable-media twins) ran a
  bare `mount`/`umount` and the two non-removable ones discarded the exit
  status, so a kernel refusal was reported as success; they had **no caller**
  anywhere in `src/` or `tests/`, so they were deleted rather than fixed.
  `create_zfs_pool` now runs `sudo zpool create` **without `-f`** — a device
  that already carries a pool signature (including an exported pool) is a
  conflict to surface, not to overwrite — and `format_device` lost its
  `ntfs` → `mkfs.ntfs` arm, which no grant sanctioned and no wizard accepted;
  it now fails as an unsupported filesystem. `list_removable_disks` parses
  `lsblk -P` through `parse_lsblk_pairs`, so a label containing a space no
  longer shifts every column (the old table split also turned an empty
  MOUNTPOINT/LABEL/UUID into a missing column), and
  `is_removable_transport` only accepts `usb`/`firewire`/`sd`/`mmc` — the
  `ata`/`sata`/`scsi`/`nvme` entries made **every internal disk** appear under
  "removable media". SMART rows are read through one parser
  (`parse_smart_attribute`) anchored on the TYPE column (`Pre-fail`/`Old_age`)
  instead of `parts[9]` (which is WHEN_FAILED as soon as the name spans two
  tokens, i.e. a failing drive read as clean), so both a two-token name and a
  `38 (Min/Max 20/45)` RAW_VALUE read correctly,
  and `get_smart_test_status` reports the drive's own advertised duration for
  that test kind (`smartctl -c`) scaled by the percent left, instead of the
  invented `pct * 2 / 10`. Every remaining `zpool`/`btrfs`/`mdadm` read goes
  through `sudo` now (`handlers/dashboard.rs` and `disk.rs::detect_disk_role`
  plus both `gather_device_facts` paths): a bare read made `zfs_member`/
  `btrfs_member` always false, so a ZFS or Btrfs member could be offered as a
  free disk for a new array. `detect_disk_role` also passes a `/dev/<name>`
  vdev path to `zpool status`, since a bare device name never matched.
- **Destroying is unmount-gated (2026-10-23).** `DiskService::delete_raid`
  resolves every mount point backed by the array from `/proc/mounts` (the
  canonical kernel node, the `/dev/md/<name>` symlink and a `UUID=…` source
  all match), unmounts each one and **aborts on the first failure** — before
  `mdadm --stop`, before the `mdadm.conf`/`fstab` cleanup, before any
  `--zero-superblock`. A second, kernel-side `umount <device>` check only
  tolerates the "not mounted" outcome. The previous code was
  `let _ = umount`, so a busy mount went on to lose its members' superblocks
  while the filesystem was still mounted and being written to (unrecoverable
  at the next remount).
- **The destroy confirmation is typed.** Storage's Virtual-disks tab asks for
  the disk name (`ConfirmDialog` `requireText`, `dlg.ask(msg, { requireText })`)
  and keeps the danger button disabled until it matches exactly — every file
  on the disk is lost and the member superblocks are erased, so the generic
  yes/no confirmation is not enough.
- `handle_encryption_create` gates `req.device` against
  `DiskService::get_available_devices()` — a past hole let an admin luksFormat
  the boot disk. `encryption::erase` runs a trailing `wipefs -a` so a deleted
  volume's device returns to the available pool, and a failure of that step is
  reported in `data.warnings[]` instead of being swallowed.
- **Encryption never erases a live volume.** `DELETE /encryption/volumes/{id}`
  re-verifies the release (`/proc/self/mounts` + `cryptsetup status`) before
  `cryptsetup erase`, and keeps the metadata row on any refusal; creation runs
  as a tracked job and rolls its own container back when a step fails. See
  [Disk Encryption](./encryption.md).
- **Editable since 2026-09-12** — the frozen-area registry is gone; the
  invariants above stay binding.

## Related

- [RAID](./raid.md), [Snapshots](./snapshots.md), [Explorer](./explorer.md),
  [Disk Encryption](./encryption.md), [Disk Cache Tier](./cache.md)
