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
  `--name <name>` (stable metadata naming) and `--bitmap=internal` on redundant
  levels (write-intent bitmap: later resyncs take seconds, not hours); instead
  of a blind 2 s sleep it polls `/sys/block/mdX/md/array_state`
  (`wait_for_array_ready`), streaming the initial-resync percent into the
  progress job. **Running out of that wait is a failure, not a shrug**
  (ROADMAP 8.2): the old code gave up after 20 s and ran `mkfs` + `mount`
  anyway, on a device the kernel was still assembling. The ceiling is now
  `ARRAY_READY_TIMEOUT` (120 s), a `clear` `array_state` fails immediately
  (the kernel holds no array — waiting cannot fix it), and a timeout fails the
  job and tears the array back down.
- `btrfs` → native `PoolService::create_pool(name, Btrfs, level, mount_point …)`
  (the `raidN` level is the Btrfs profile 1:1), run in `spawn_blocking`,
  mounted under `RAID_MOUNT_BASE/<name>` like the mdadm arrays.
- `zfs` → `create_zfs_pool_from_devices` (arg order: `zpool create -m
  <mountpoint> [-O props…] <name> [vdev-type] <devices…>` — the name comes
  **before** the vdev type, and **no `-f`**: forcing creation would overwrite a
  pool signature the free-device gate already refused, ROADMAP 8). The root
  dataset is created with the appliance's standard properties
  (`compression=lz4`, `atime=off`, `acltype=posixacl`, `xattr=sa`) so the
  shares' ACL tooling works identically. **raid10 is a stripe of mirrors** (one
  `mirror` vdev per device pair) — it used to mis-map to a single raidz3
  (2026-10-19 fixed).

Extra mdadm options (`ext4`/`xfs` techs only): `spares` attaches extra checked
disks as **hot spares** (`--spare-devices`) and `allow_missing` starts the
array **deliberately degraded** — mdadm `missing` placeholders fill up to the
level's budget. That budget is **one table** (`max_missing_for_level`) read by
both the API gate and the job, so the two can never disagree: raid1/5 → 1,
raid6 → 2, and **raid0/raid10 → 0**. The raid10 refusal is deliberate, not an
oversight: mdadm only places a `missing` slot in RAID 10 with an explicit
`--layout`, and the slot placement then depends on the layout it picks — the
wizard never showed that geometry, so the request is refused with that reason
in the message. Native ZFS/Btrfs pools strip every disk they get: spares and
missing are rejected for those technologies.

Progress milestones flow through the `CreateProgress` trait into the job
(validation → array creation → resync percent → format → mount/persist →
done); a failure emits a `failed` progress event, never a `done` one.

**Persistence is verified, not hoped for.** The fstab edit is serialized
behind `PERSIST_LOCK`, and after every append `sudo findmnt --verify --fstab`
re-parses the file; a verification failure rolls the appended line back and
fails the creation instead of leaving a line that would break the next boot.
After the `mdadm.conf` append the initramfs is regenerated best-effort
(`sudo update-initramfs -u`, `sudo dracut -f` fallback; both are scoped
sudoers grants) so the new array assembles early in boot. The `mdadm.conf`
append itself is the one persistence step that cannot be rolled back (the
array is already created, formatted and mounted), so it is never silent: a
failed/empty `mdadm --detail --scan` or a failed append is logged at error
level with the consequence spelled out — the array will not reassemble after
a reboot — plus the exact `ARRAY` line to add by hand (ROADMAP 7.4). Every
privileged
step of that path is scoped in the sudoers policy (ROADMAP 7.1/7.4): the fstab
and `mdadm.conf` writes are `tee /etc/fstab` / `tee /etc/mdadm/mdadm.conf`, the
mount point is `mkdir -p /mnt/*`, the mount itself is
`mount [-o <opts>] <dev> /mnt/*`, `mdadm` is granted per mode (`--create`,
`--detail`, `--examine`, `--stop`, `--zero-superblock`, `--manage`, `--grow`,
`--action`/`--action=<mode>`), and the teardown's unmount is
`umount /mnt/*` (plus `/dev/*` for the kernel-side check).

**A failed setup only undoes its own array (ROADMAP 8.1).** When the format or
mount step fails, `teardown_failed_array` re-reads the array's identity through
`mdadm --detail` and compares it with the identity captured right after *this
job's* `--create` (metadata label pinned by `--name`, plus the superblock UUID
when mdadm prints one). The plan is conservative on purpose:

| Situation | What the teardown does |
|---|---|
| the array no longer exists | nothing |
| label or UUID differ (the node was recreated under the same name) | nothing, and an `error!` names why |
| the identity could not be captured at create time | stop the array, **erase no superblock** |
| the identity matches | stop the array, then zero only the devices that are *both* in the request *and* still reported as members |

The previous version ran `--zero-superblock` on **every** device of the
request, with no identity check: a device claimed by another array in the
meantime — or one that belonged to an exported array — lost its metadata as a
side effect of an unrelated formatting error. Two further guards came with it:
`mdadm --stop` must **succeed** before anything is wiped (a stop that fails
means the array is still live, typically still mounted — which is exactly the
case when the failure happened *after* the mount), and a non-zero exit from
`--zero-superblock` is now reported instead of being mistaken for a spawn
error. A failed `mdadm --create` goes through the same teardown, so a
half-assembled array (metadata written on the first devices, failure on the
fourth) is cleaned up instead of leaving superblocks that block the retry.

**Every privileged call is bounded (ROADMAP 8.3).** `mdadm`, `mkfs.*`, `mount`,
`blkid`, `findmnt`, `zpool create`, `sudo tee` and the initramfs hooks all run
through one `run_timed` helper (`kill_on_drop`, so a timed-out tool really
dies) with the deadlines in `src/disk.rs` (`MDADM_TIMEOUT` 120 s,
`MKFS_TIMEOUT` 900 s, `MOUNT_TIMEOUT` 60 s, `BLKID_TIMEOUT` 30 s,
`ZPOOL_TIMEOUT` 120 s, `INITRAMFS_TIMEOUT` 300 s). Without them a hung tool
held `CREATE_LOCK` for the life of the process: every later create answered
"already running" with no way out short of a service restart.

**Per-user quotas are armed at creation time.** `mount_and_persist_raid`
mounts the new `ext4`/`xfs` array with `QuotaService::quota_mount_options(fs)`
(`usrquota,grpquota`, or `usrquota` for XFS) so the kernel accounts per-user
usage from the first write. If that mount fails the array is re-mounted
plainly (a warning is logged and the home-folder limit is reported as not
enforced), and the quota options are written to `/etc/fstab` **only** when the
live mount accepted them — a persisted option the kernel rejected would break
the next boot. `activate_mount` then runs `quotacheck -cugm -f` + `quotaon -u`
on the array. See [Disk Quotas](./quotas.md).

Tool failures come back as `DiskError::CommandFailed` with the stderr passed
through `friendly_tool_error` — known patterns (busy, too small, in use,
permission…) become a short sentence kept readable in a dialog, and always
carry the tool's own last error line. A tool that outlives its deadline is a
distinct `DiskError::Timeout` ("did not finish within Ns"), because it may
still have been doing the right thing: the message must not read as a refusal.

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

The old `GET /storage/raid` (list) route still exists for compatibility, and
`POST /storage/raid/{name}/rebuild` is reused by Storage's Rebuild button.
Listing/destroying is done through `GET|DELETE /api/storage/virtual-disks`
(see [Storage](./storage.md)); the legacy `DELETE /storage/raid/{name}` was
removed on 2026-10-23 so the RAID surface really is creation-only (it had no
UI caller and contradicted this page's own boundary statement).

Health surfaces arrays too: the `raid_arrays` check (ids `health.raid_arrays`)
reports degraded/failed mdadm arrays as a warning/error, and raises
notifications through the standard health-event catalog.

## Deadlines on the privileged tools (2026-10-24, ROADMAP X.2)

`disk.rs` already bounded the virtual-disk *create* path (ROADMAP 8.3); the
2026-10-24 pass bounded **everything else** in the module — the scans and
probes the RAID screen runs on every poll: `lsblk`, `df`, `sudo mdadm --detail
--scan`, `sudo mdadm --examine`, `sudo zpool status`, `sudo btrfs filesystem
show` and every `smartctl` read. `run_timed`/`bounded` now delegate to
`crate::utils::process::run_timed_async_result` (one implementation of
"kill the child at the deadline"), so a wedged probe is a named
`DiskError::Timeout` naming the program, never a `spawn_blocking` thread held
for the life of the process. `sudo tee` — how `/etc/fstab` and
`/etc/mdadm/mdadm.conf` are persisted — goes through
`run_timed_async_with_stdin`, which keeps the deadline **and** the stdin the
plain helper closes (a closed stdin would make `tee` truncate the file to
nothing). The deadline table lives in
[storage.md](./storage.md#deadlines-on-every-host-tool-2026-10-24-roadmap-x2).

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
- Every disk-touching command in `disk.rs` / `pools.rs` stays `sudo`-wrapped,
  and every one of them carries a deadline (`run_timed`) — a hung `mdadm` must
  never outlive the request that started it.
- `zpool create`'s pool name precedes the vdev type, and the command carries no
  `-f`: a device that already holds a pool signature is a conflict to report,
  never one to overwrite.
- **A failed setup only touches its own array.** The teardown compares the
  identity captured after its own `--create` with what `mdadm --detail` reports
  now, and zeroes only the intersection of (request members) and (current
  members). No identity → no wipe. `mdadm --stop` must succeed first.
- **An array that never becomes usable fails the creation.** `array_state`
  never reaching `clean`/`active`/`active-idle`/`readonly`/`read-write` within
  `ARRAY_READY_TIMEOUT` is an error, not a reason to format anyway.
- The mdadm member-device parse reads the detail table's last token (a
  `/dev/…` path), not `parts.first()`, so `removed`/`missing` rows never become
  wipe targets.
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

## What is *not* covered by tests

Stated here rather than implied, because "the suite is green" overstates
readiness for this app (ROADMAP 8):

- **No test performs a real `mdadm --create` / `mkfs` / `mount`.** That path
  needs root and real block devices; the only lifecycle coverage is the opt-in
  E2E §13 of `raid.spec.ts` (`RAID_E2E_DEVICES`), which runs on a host with
  spare disks and is skipped everywhere else.
- `tests/raid_integration.rs` covers the auth matrix plus the synchronous
  request-shape refusals (including the degraded-create policy) — it never
  reaches the job.
- The teardown's decision (`plan_teardown`), the array-state classification, the
  resync percentage, the `mdadm --detail` parse and the command deadlines are
  unit-tested (`src/disk.rs`); what is *not* proven is their effect on a real
  kernel. The failure modes they prevent (wipe a foreign device, format an
  assembling array, wedge `CREATE_LOCK`) are reasoned from the code, not
  reproduced.
- `zpool create`'s argv (pool name before the vdev type, no `-f`, one `mirror`
  group per pair for raid10) is unit-tested through `zfs_create_args`; what is
  not proven is zpool's own reaction on a host that holds an exported pool.

## Related

- [Storage](./storage.md), [Snapshots](./snapshots.md), [Rescue](./rescue.md),
  [API Contract](../API.md)
