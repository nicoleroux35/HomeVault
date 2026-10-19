# RAID — virtual-disk creation wizard

**Purpose:** A **creation-only** wizard that turns physical disks into one
**"virtual disk"**. Everything else — listing, verifying/scrubbing, rebuilding,
**growing onto a new disk**, destroying — lives in the [Storage](./storage.md)
app's "Virtual disks" tab.

> The RAID/Storage split was **inverted** on 2026-09-06. The words
> "pool"/"grappe"/"array" are out of the RAID UI/i18n; RAID never lists or
> destroys anything.

## Backend

`handle_raid_create` + `handle_raid_available_devices` in
`src/handlers/storage.rs`. Creation runs as a **tracked background job**:

1. The handler validates synchronously (pure `validate_virtual_disk_request`:
   name/level/filesystem rules, level minimums, spare/degraded rules, raid10
   even-member rule), runs the device gate, and checks the name is not already
   claimed (`array_name_taken`) — any violation answers 400 before a job
   exists.
2. It creates a progress job (`JobType::Custom("raid_create")`),
   **returns `{message, job_id}` immediately**, and spawns the real work.
3. The background task holds `DiskService::CREATE_LOCK` (single-flight: two
   concurrent creates settling on the same free disk cannot both pass the
   gate), re-checks the validation + gate (close the validate-then-use
   window), and drives `DiskService::create_virtual_disk`.

`create_virtual_disk` is the **single dispatch point** by target filesystem:

- `ext4` / `xfs` → mdadm via `DiskService::create_raid`, then
  `mount_and_persist_raid` writes `/etc/fstab` + `/etc/mdadm/mdadm.conf` so the
  array reassembles after reboot. The create command carries
  `--name <name>` (stable metadata naming), `--bitmap=internal` on redundant
  levels (write-intent bitmap: later resyncs take seconds, not hours) and
  instead of a blind 2 s sleep polls `/sys/block/mdX/md/array_state`
  (`wait_for_array_ready`), streaming the initial-resync percent into the
  progress job.
- `btrfs` → native `PoolService::create_pool(name, Btrfs, level, mount_point …)`
  (the `raidN` level is the Btrfs profile 1:1), run in `spawn_blocking`,
  mounted under `RAID_MOUNT_BASE/<name>` like the mdadm arrays.
- `zfs` → `create_zfs_pool_from_devices` (arg order: `zpool create -f -m
  <mountpoint> [-O props…] <name> [vdev-type] <devices…>` — the name comes
  **before** the vdev type). The root dataset is created with the appliance's
  standard properties (`compression=lz4`, `atime=off`, `acltype=posixacl`,
  `xattr=sa`) so the shares' ACL tooling works identically. **raid10 is a
  stripe of mirrors** (one `mirror` vdev per device pair) — it used to mis-map
  to a single raidz3 (2026-10-19 fixed).

Extra mdadm options (`ext4`/`xfs` techs only): `spares` attaches extra checked
disks as **hot spares** (`--spare-devices`) and `allow_missing` starts the
array **deliberately degraded** — mdadm `missing` placeholders fill up to the
level's budget (raid1/5: 1, raid6: 2; raid0/10 are refused). Native ZFS/Btrfs
pools strip every disk they get: spares/missing are rejected for those
technologies.

Progress milestones flow through the `CreateProgress` trait into the job
(validation → array creation → resync percent → format → mount/persist →
done); a failure emits a `failed` progress event, never a `done` one.

**Persistence is verified, not hoped for.** The fstab edit is serialized
behind `PERSIST_LOCK`, and after every append `sudo findmnt --verify --fstab`
re-parses the file; a verification failure rolls the appended line back and
fails the creation instead of leaving a line that would break the next boot.
After the `mdadm.conf` append the initramfs is regenerated best-effort
(`sudo update-initramfs -u`, `sudo dracut -f` fallback; both are scoped
sudoers grants) so the new array assembles early in boot. If the format or
mount step fails, `teardown_failed_array` stops the fresh array and zeroes the
member superblocks (logging every step) instead of leaving an orphan holding
the disks busy.

Tool failures come back as `DiskError::CommandFailed` with the stderr passed
through `friendly_tool_error` — known patterns (busy, too small, in use,
permission…) become a short sentence kept readable in a dialog, and always
carry the tool's own last error line.

Before dispatching, `handle_raid_create` runs the server-side device gate
`DiskService::validate_pool_devices`: every candidate device must exist, must
not host the system disk, and must be **entirely free** — no partitions, no
filesystem signature, not mounted, no mdadm/ZFS/Btrfs superblock — else 400.
`mdadm --create --run` skips mdadm's own "suitable as boot device?"
confirmation, so the overwrite of an occupied disk would otherwise be silent
(2026-10-19: the endpoint used to check device *existence* only, and the
"free" verdict lived only in a buggy frontend filter). The rebuild path
(`replacement_device`) and the new grow endpoint are gated the same way.

The device list itself comes from `get_available_devices` — **every**
candidate (non-system) disk is listed now, not only the free ones: in-use
disks are reported dimmed with an English `reason` (the first fact that
disqualifies them) and an overall SMART read (`smart_status`: `Healthy` /
`Failed` / `Unknown` via one `smartctl -H` probe). The busy/in-free verdict is
judged from a **single** full-tree `lsblk` pass (`PKNAME` adjacency, no more
per-disk lsblk shell storm); only the mdadm/ZFS/Btrfs membership probes remain
per disk, and only when the tree itself doesn't already disqualify it.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /storage/raid/devices` | auth | candidate physical disks (`inUse`, `reason`, `smart_status`) |
| `POST /storage/raid` | admin | create one virtual disk as a background job → `{message, job_id}`; body takes `spares?` / `allow_missing?`; device-is-free + name-collision + request-shape gates enforced server-side |
| `POST /storage/virtual-disks/{name}/grow` | admin | mdadm only: `--add` the device then `--grow --raid-devices N+1` (`grow_raid`); raid0/raid10 refused; the reshape runs in the background |

The old `GET /storage/raid` (list), `DELETE /storage/raid/{name}`, and
`POST /storage/raid/{name}/rebuild` routes still exist — Storage's "Virtual
disks" tab reuses the rebuild route; listing/destroying is done through
`GET|DELETE /api/storage/virtual-disks` (see [Storage](./storage.md)).

Health surfaces arrays too: the `raid_arrays` check (ids `health.raid_arrays`)
reports degraded/failed mdadm arrays as a warning/error, and raises
notifications through the standard health-event catalog.

## Frontend

- `frontend/src/routes/Raid.svelte` — a 3-step wizard rendered inline (Type ·
  Disks · Confirm), re-skinned to the mono house style 2026-09-07. E2E hooks:
  `.raid-root`, `.rl-wizard-card`, `.rl-type-grid`, `.rl-type-card`,
  `.rl-progress-track > span`, `.rl-disk-row.selectable`, `.rl-fs-card`,
  `#raidName`. During the tracked job the footer shows the milestone +
  percent bar and locks the wizard; the in-use rows stay listed (dimmed,
  non-interactive, with the server's `reason` and a SMART chip). Beyond the
  level minimum the wizard offers "hot spares" for the extra disks; below it,
  a "create degraded" opt-in for mdadm redundant levels.
- `frontend/src/lib/services/api.ts`: `getRaidAvailableDevices`, `createRaid`
  (spares / allow_missing), `growVirtualDisk`, `subscribeToProgress`.
- `frontend/src/routes/Storage.svelte`: "Add disk" on mdadm cards — a modal
  lists the free devices (`grow` endpoint).
- `frontend/src/routes/Health.svelte`: a 9th check card (`raid_arrays`).

## Invariants

- Every RAID endpoint requires a **valid session** (a past bug only checked the
  `X-Session-Token` header was non-empty).
- Every disk-touching command in `disk.rs` / `pools.rs` stays `sudo`-wrapped.
- `zpool create`'s pool name precedes the vdev type; the mdadm member-device
  parse reads the detail table's last token, not `parts.first()`.
- No "pool"/"grappe" wording in RAID's UI or i18n; RAID never lists or destroys.
- Real end-to-end run on loop devices (2026-09-07) fixed 4 bugs in `src/disk.rs`
  (`mount`/`umount`/`blkid` were not `sudo`-wrapped; `zpool create` arg order).
- **A declared-free device must really be free.** The free/in-use verdict is
  facts-based (`facts_indicate_in_use`, unit-tested), never a role-string
  comparison: the hardcoded-`"Available"` role check made every disk report
  `inUse: true` and the wizard's step 2 permanently empty (2026-10-19). The
  device-is-free gate is also enforced **server-side** in `handle_raid_create`
  (`validate_pool_devices`); the wizard's own list is never the only guard.
- **E2E step 2/3 don't skip blind.** The disk-step tests of `raid.spec.ts` gate
  on `requireFreeDisks`: plain hosts skip, but an opt-in `RAID_E2E_DEVICES`
  run *fails* when the provided devices don't show up as selectable rows —
  silent skips were what kept the "every disk inUse: true" bug invisible.
- **Creation is a job, tests wait for it.** `POST /storage/raid` answers
  before the array exists, so the opt-in lifecycle tests (13.1/13.3) poll the
  virtual-disks list for the new name (30 s ceiling) before asserting.
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Raid.svelte`, the RAID api.ts methods, the RAID handlers in `storage.rs` and
  the `raid.*` i18n section are open to edits. The invariants above stay
  binding: a change keeps the Rust integration + Playwright E2E suites green.

## Related

- [Storage](./storage.md), [Snapshots](./snapshots.md), [Rescue](./rescue.md),
  [API Contract](../API.md)
