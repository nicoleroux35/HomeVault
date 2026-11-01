# Disk Quotas

**Purpose:** the per-user home-directory disk-space limit, enforced by the
kernel on a real filesystem, and reported truthfully in the UI (enforced vs
not enforced, with the reason and the usage read back from disk).

This is distinct from **[Transfer Quotas](./transfer-quotas.md)**
(per-user/group upload/download byte caps enforced against File Explorer
transfers) — same word, different systems, different backing files.

> **Removed 2026-09-09 — per-group filesystem quotas.** The old
> `setquota -g` feature (admin CRUD under `/api/quotas`,
> `src/handlers/quotas.rs`, `src/storage/quota_repo.rs`,
> `tests/quotas_integration.rs`, the group-quota methods of `QuotaService`,
> and the Settings **"Disk Quota" card**) was deleted: disk quotas are managed
> per user here and per user/group (transfer metering) via
> [Transfer Quotas](./transfer-quotas.md). The `quotas` DB table from its
> migration is left in place (migrations are immutable) but unused.

## Scope and the one invariant

A home-folder size limit is only worth anything if the kernel refuses writes
past it. This feature therefore never reports a limit as "applied" just because
it was **stored**: every write is followed by a read-back, and the UI shows
exactly two outcomes — **enforced** or **not enforced with a reason**. A third,
ambiguous state ("saved") is not acceptable and is a bug.

What is enforced where:

| Filesystem | Backend | How |
| --- | --- | --- |
| `ext2` `ext3` `ext4` `jfs` `reiserfs` | VFS (`quota-tools`) | mount options `usrquota,grpquota` + `quotacheck`/`quotaon`, limit via `setquota -u`, read-back via `repquota` |
| `xfs` | VFS (`quota-tools`) | mount option `usrquota` (accounting comes from the option; XFS has no quota files), limit via `setquota -u`, read-back via `repquota` |
| `zfs` | native | property `userquota@<user>` on the dataset, read-back via `zfs get` |
| `btrfs`, `vfat`, `exfat`, `ntfs`, network filesystems, anything else | none | reported `Unsupported` — the limit is stored but never claimed to be enforced |

Btrfs is deliberately excluded: its qgroups are subtree-based and there is no
uid/gid quota interface, so `setquota` cannot work there. Never add it to the
support matrix without a real enforcement path.

## Per-share quotas (2026-10-31, QW-07)

The guide above covers **per-user** quotas (a limit on a person's home). This is
the other axis DSM calls *shared-folder quota*: a ceiling on **one share**, so a
single runaway folder cannot fill the pool.

`src/share_quota.rs` (backend selection, argv, parsers, the decision),
`src/storage/share_quota_repo.rs` (`share_quotas`, migration **066**),
`src/handlers/share_quota.rs` (endpoints).

A share has no row of its own — it is derived from `smb.conf` / `/etc/exports`
(`SharesService::get_shares`) — so the split is: **the intent lives in
`share_quotas`, the enforcement lives in the filesystem.**

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /api/shares/{name}/{share_type}/quota` | admin | `{ backend, supported, filesystem, reason, quota_bytes, used_bytes, free_bytes, percent, quota_human, used_human, min_bytes, max_bytes }` |
| `PUT /api/shares/{name}/{share_type}/quota` | admin | `{ quota_bytes }`; `0` removes the limit. The filesystem is changed **first** — the row only records what it accepted |
| `DELETE /api/shares/{name}/{share_type}/quota` | admin | removes the limit (filesystem, then row) |

### Which filesystems can enforce one

| Filesystem | Mechanism | Notes |
|------------|-----------|-------|
| **ZFS** | `zfs set refquota=<bytes>` on the dataset | native, independent of any per-user quota |
| **Btrfs** | `btrfs qgroup limit <bytes> <subvolume>` | only when the share **is** a subvolume; a plain folder inside one is refused, because a qgroup limit would cap every share in that subvolume |
| **XFS** | project quota (`xfs_quota … limit -p bhard=…`) | requires `prjquota`/`pquota` in the mount options; the project id is derived deterministically from the share path (never from a counter, so it survives restarts) |
| ext4 / FAT / network mounts | — | **`supported: false`** with the reason, and the card shows the reason instead of a field |

The detection reuses the same `findmnt` probe as the per-user quotas, and the
selection is a **pure function** (`backend_for`) whose whole matrix is
unit-tested — including the two refusals that matter (a plain folder in a Btrfs
subvolume, an XFS mount without `prjquota`).

### Usage is read back, never guessed

| Backend | Read |
|---------|------|
| ZFS | `zfs list -H -p -o used <dataset>` (raw bytes) |
| Btrfs | `btrfs qgroup show --raw <path>`, the share's own `0/<id>` row (`rfer`) |
| XFS | `xfs_quota -x -c "report -p -b -N"`, the project's block count × 1024 |

Every command runs through `run_timed` with a 10 s deadline and on the blocking
pool, like the rest of the quota code (ROADMAP X.1/X.2).

### Where enforcement starts

The Explorer's **upload** and **copy** paths check the destination share
(`handlers/explorer.rs::enforce_share_quota`):

- **Upload** — once before the first byte (an obviously-full share refuses
  immediately) and once after each file lands. The second check is per *file*,
  not per chunk: each check is a `zfs list`/`btrfs qgroup`/`xfs_quota` probe, and
  a per-chunk probe would cost more than the upload. A file that pushes the share
  past its ceiling is **deleted** and the request refused, so the share is never
  left over its limit.
- **Copy** — the source's real size (`stat`, or a recursive walk for a tree) is
  compared before anything is written, so the answer is "it does not fit", not
  "try and see".
- The refusal is `507 Insufficient Storage` with the three numbers in the
  sentence (`2.5 GB of 10 GB used, and this transfer needs 9 GB`).
- A protocol client writing over SMB/NFS is bounded by the **filesystem** limit
  itself — that is the whole point of `refquota`/qgroups/project quotas — so the
  two enforcement points are complementary, not duplicated.

The decision itself is pure:

```rust
pub fn would_exceed(used_bytes: u64, quota_bytes: u64, incoming_bytes: u64)
    -> Result<(), QuotaExceeded>
```

`quota_bytes == 0` means "no limit" and can never be exceeded; a transfer that
lands **exactly** on the ceiling is allowed (the quota is the ceiling, not one
byte below it); and the sum saturates rather than wrapping.

### The screen

Shares → each card has a **Size quota** button (an `IconGauge`, with an
`aria-label`, never a bare `title`). With a ceiling set the card shows
`2.5 GB of 10 GB` and a `role="progressbar"` bar (`aria-valuenow`/`-min`/`-max`,
a translated `aria-label`) that turns warning at 80 % and error at 95 %. The
editor takes **GB** (what an admin thinks in) and converts to bytes; `0` or
**Clear** removes the limit. When the filesystem cannot enforce one, the card
shows `A size quota cannot be enforced on ext4 — <reason>` instead of a field.

## Backend

`src/quota.rs` — `QuotaService`, no repository, no state:

| Item | Role |
| --- | --- |
| `backend_for_path(home_path)` | asks the kernel mount table (`findmnt -n -J -o FSTYPE,SOURCE,OPTIONS,TARGET --target <path>`) which backend serves the folder. Picking the mount by path (not by the disk label in the DB) means a home folder on a sub-mount is quotad against that sub-mount, not `/`. |
| `backend_for(fs, source, options, mount_point)` | pure backend matrix (unit-tested without quota tooling on the host). |
| `inspect(user, path, limit)` | read-only: detects the backend and reports the live state. No `sudo`, no mutation. |
| `apply(user, path, limit)` | applies (or clears) the hard limit, then **verifies it with a read-back** and returns that state. |
| `activate_mount(fs, mount_point)` | `quotaon -u <mount>`; on failure runs `quotacheck -cugm -f <mount>` then retries. Called after a pool/RAID array is mounted with quota options and by the installer. No-op on XFS. |
| `activate_all()` | `quotaon -aug` once at startup, so a reboot cannot silently turn enforcement off (`src/web/mod.rs`). |
| `quota_mount_options(fs)` | mount options that make the kernel account per-user usage, or `None` when the filesystem cannot (`disk.rs`, installer). |
| `privileged_argv` / `run_privileged` | every privileged call goes through `sudo -n` (see below). |

Read-backs:

- VFS: `repquota -u -p <mount>` parsed by `parse_repquota` (handles the optional
  grace column and `-`/0 sentinels; block counts are 1-KiB).
- ZFS: `zfs get -H -p -o value userquota@<user> <dataset>` and
  `zfs userspace -H -p -o name,used <dataset>`.

`enforced` is `true` only when the read-back confirms the configured limit (for
an unlimited home, when accounting is actually active). A limit mismatch — the
filesystem reports a different value than the one configured — is reported as
**not enforced** (`limit_mismatch`), never rounded into success.

### Privileges and timeouts

Every privileged command runs as `sudo -n <cmd>`, with a deadline:

| Operation | Timeout |
| --- | --- |
| `findmnt`, `repquota`, `zfs get`, `quotaon -u` | 10 s |
| `setquota`, `zfs set userquota` | 20 s |
| `quotacheck`, `quotaon -aug` | 120 s |

`-n` (non-interactive) is deliberate: a missing privilege grant fails fast
with a clear error instead of hanging the request on a password prompt. On a
real install there is nothing to grant — the service runs as **root** (no
`User=` in the systemd unit) and `sudo -n` succeeds against the distro default
`root ALL=(ALL:ALL) ALL`, exactly like every other privileged shell-out
(`src/setup.rs::configure_sudoers` still lists a scoped `NOPASSWD` line for the
four quota tools, but that file is not installed today; it is kept for a
future non-root mode and documents the intended scope). Sudoers cannot
constrain numeric arguments, so such a rule covers the binaries, not their
arguments — which is why the arguments are built by `setquota_args` /
`zfs_quota_value` rather than assembled from user input.

`home_path` and the system user name are validated (`valid_system_username`)
before they reach a command line.

### Never blocking the request

The commands run through `tokio::process` with `kill_on_drop`, so a stuck
`quotacheck` on a huge array cannot pin a request thread; the API answers with
the state it has (not enforced + `command_failed`) once the deadline hits.

## Reason codes

`QuotaState.reason_code` is a stable snake_case enum (`QuotaReason`) that the
frontend localises into all four languages; `QuotaState.reason` keeps the
English diagnostic sentence for logs and as a last-resort fallback.

| `reason_code` | Meaning |
| --- | --- |
| `not_mounted_with_quota` | the filesystem supports quotas but was mounted without `usrquota`, so the kernel cannot enforce anything |
| `unsupported_filesystem` | no per-user quota interface (Btrfs, vfat, network filesystems, …) |
| `accounting_inactive` | accounting is off / the report could not be read on this mount |
| `no_entry` | accounting is on, but this account has no row yet (e.g. no usage recorded) |
| `limit_mismatch` | the filesystem reports a different limit than the one saved |
| `command_failed` | `setquota`, `quotaon`, `quotacheck` or `zfs` failed |
| `inspect_failed` | the mount serving the home folder could not be inspected |
| `invalid_username` | the account name cannot be used in a quota command |

## Where enforcement starts

1. **Pool/RAID creation** (`src/disk.rs::mount_and_persist_raid`): the array is
   mounted with `quota_mount_options(fs)`; if the mount with quota options
   fails, it falls back to a plain mount (warning logged, limit reported as not
   enforced) and the quota options are written to `/etc/fstab` **only** when
   the live mount accepted them. Then `activate_mount` runs `quotacheck` +
   `quotaon`, so the array enforces from the first byte written.
2. **Installer** (`installer/install.sh::setup_home_quota`): for the existing
   `/home` filesystem, edits `/etc/fstab` in place (via `cat`, keeping inode and
   permissions), validates with `findmnt --verify --fstab`, rolls back on
   failure, remounts live, then `quotacheck -cugm -f` + `quotaon -ug`. ZFS
   datasets are left to the native backend; Btrfs and other filesystems are
   reported as unsupported. Step 4 of the installer now requires all four
   quota binaries.
3. **Saving a home folder** (`src/handlers/user_homes.rs::apply_home_folder`):
   `QuotaService::apply` runs and, when the result is not enforced, the
   response carries a truthful warning (and the state is shown in the UI).
   Clearing a limit is `apply(..., None)` — it removes the quota row rather
   than leaving a stale one behind.
4. **Startup** (`src/web/mod.rs`): `QuotaService::activate_all()`.

## API

`GET /api/users/{username}/home` (any authenticated user; the handler checks
the session) returns the stored config plus a live `quota` object:

```json
{
  "enabled": true,
  "disk": "/mnt/pools/data",
  "path": "/mnt/pools/data/users/jsmith",
  "created_at": "…",
  "size_bytes": 10737418240,
  "quota": {
    "enforced": false,
    "filesystem": "ext4",
    "mount_point": "/mnt/pools/data",
    "limit_bytes": 10737418240,
    "used_bytes": 2147483648,
    "reason_code": "not_mounted_with_quota",
    "reason": "The ext4 filesystem mounted on /mnt/pools/data was not mounted with 'usrquota', so the kernel cannot enforce a per-user limit"
  }
}
```

`quota` is `null` when the home folder is disabled or absent — the endpoint
never invents a state. `PUT /api/users/{username}/home` is admin-only and
applies the limit; `DELETE` removes the home config.

## Frontend

The Users detail sheet's home card (`frontend/src/routes/Users.svelte`):
the size slider, plus the quota status block (`data-testid="users-home-quota"`)
which renders

- the limit + mount and the read-back usage when enforced,
- "no limit, accounting is active on %mount%" when the home is unlimited but
  the kernel is accounting usage,
- a warning with the localised cause when it is not enforced,
- nothing when there is no home folder at all.

`quotaReasonKey()` maps `reason_code` to an i18n key in `en`/`fr`/`de`/`es` and
falls back to the server's English `reason` for an unknown code, so a future
backend variant can never blank the explanation. After a save the card is
refilled from `refreshAll()` → `hydrateDetail()` → `loadHomeQuota()`, never
from the form, so a limit the kernel refused cannot show up as success.

## Tests for per-share quotas

- `src/share_quota.rs` — **22 unit**: the backend matrix (ZFS → `refquota`,
  Btrfs subvolume → qgroup, Btrfs **plain folder refused** with the reason
  naming "every share", XFS only with `prjquota`/`pquota` and refused without,
  four unsupported filesystems each naming itself and the alternatives, empty
  fstype → `unknown`); the project id being stable, path-normalised, different
  per share and never in XFS's reserved range; the argv for every backend
  (ZFS `refquota=<bytes>` / `none`, Btrfs `qgroup limit <bytes|none>`, XFS
  `project -s -p … <id>` then `limit -p bhard=<blocks>` with blocks rounded
  **up**); the parsers on real-shaped output (`zfs list -p`, `btrfs qgroup
  show --raw`, `xfs_quota report -p -b -N`, `btrfs subvolume show`, `findmnt
  -J`) including the "not found" answers that must stay `None` rather than
  guess; the decision (`would_exceed`: fits, lands exactly on the ceiling,
  crosses it with its three numbers, `0` never full, an already-full share
  refusing a 1-byte file, no wraparound at `u64::MAX`) and the **acceptance
  criterion verbatim** (11 GB into a 10 GB share is refused with both numbers in
  the sentence); validation floor/ceiling; `human_size` rendering.
- `tests/share_quota_integration.rs` — **5**: the admin matrix on all three
  verbs; an unknown share named in the refusal and an unknown protocol refused
  as such; the validation floor and ceiling refused **with nothing stored**; the
  declared limit round-tripping in the database (per (name, type), `0` deletes
  the row, `clear_all_for` dropping every protocol's row); and the state payload
  carrying every key the card needs, with an unsupported backend **always**
  explaining itself and claiming `quota_bytes: 0`.
- `tests/e2e/shares/quota.spec.ts` — **9 `test(`**: the quota action on every
  share card with an accessible name; the editor opening with a real `<label>`
  and cancelling; a set ceiling rendering `used of quota`, the percentage and a
  `role="progressbar"` carrying `aria-valuenow`/`-max`/`-label`; an unsupported
  filesystem explaining itself (naming `ext4` and what does work) with **no**
  bar; saving sending **bytes** while the editor speaks GB, and the bar
  appearing from the answer; a refused save as `role="alert"`; a member refused
  on read and write; no duplicate ids with the editor open; no horizontal
  overflow with the bar and editor open. Every write is **mocked**.

**No test applies a real filesystem limit.** That needs ZFS/Btrfs/XFS with root
and would modify the host; the argv and parsers are unit-tested, the endpoint
contract is integration-tested, and the screen is E2E-tested with the endpoints
intercepted.

## Tests

- Unit (`src/quota.rs`): backend matrix, `usrquota` detection, `findmnt` JSON
  parsing, `repquota` parsing (grace column, sentinels), ZFS value/userspace
  parsing, `setquota` argument construction, `sudo -n` argv, byte↔KiB
  conversion, and `reason_code` wire serialisation.
- Integration (`tests/quota_integration.rs`): the HTTP contract — a stored
  limit is reported **not enforced** with a `reason_code`, an uninspectable
  path yields `inspect_failed`, no home folder yields `quota: null`, and the
  auth/validation rules hold.
- E2E (`tests/e2e/users/users.spec.ts` §18): the rendered states — enforced vs
  not enforced with reason and usage, the unknown-code fallback, the French
  localisation, and the "save does not flip a false green" case.

Live enforcement (a filesystem really mounted with `usrquota` and a real
account) is a host property, not something a test can arrange; it is verified
manually on a real install and by the E2E host when quota tooling is present.

## Related

- [Users](./users.md), [Settings](./settings.md),
  [Storage](./storage.md), [Transfer Quotas](./transfer-quotas.md)
