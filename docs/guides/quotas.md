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
