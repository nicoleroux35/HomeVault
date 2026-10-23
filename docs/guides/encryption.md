# Disk Encryption (LUKS2)

**Purpose:** Encrypt a single available (unassigned) block device with LUKS2.
Deliberately scoped to standalone devices — not RAID arrays (those stay on the
[RAID](./raid.md) page) and not ZFS pools (ZFS already has native
dataset encryption, a separate mechanism). Unlock is always **manual**: no
key material is ever stored on the server, so a volume stays locked after
every reboot until an admin re-enters the passphrase through the UI.

## Backend

- `src/encryption.rs` — shells out to `cryptsetup` (`luksFormat`, `open`,
  `close`, `status`, `luksUUID`, `luksChangeKey`, `erase`) plus `mount`/`umount`
  and `wipefs`. Passphrases are always piped via **stdin** (`--key-file -`),
  never passed as a CLI argument, so they never appear in the process list.
- `src/handlers/encryption.rs` — HTTP handlers (see [API Contract](../API.md)
  for the endpoint table).
- `src/storage/encrypted_volume_repo.rs` — `encrypted_volumes` table
  (migration `014_encrypted_volumes.sql`). **The passphrase is never persisted**
  — only `device_path`, `mapper_name`, `luks_uuid`, `filesystem`, `label`, and
  `mount_point` are stored, enough to address the volume once it's unlocked by
  a human. This is a deliberate improvement over the plaintext-password
  pattern used by `restic_repos`, deliberately not copied here.
- Device discovery reuses the existing
  `DiskService::get_available_devices()` from `src/disk.rs` (the same pool the
  RAID page's "available devices" picker uses), filtered to `in_use == false`.
- Formatting reuses the existing `DiskService::format_device()`
  (same `mkfs.ext4`/`mkfs.xfs`/`mkfs.btrfs` dispatch RAID already uses) — run
  against the `/dev/mapper/<mapper_name>` device, not the raw disk.
- fstab persistence: a `noauto,defaults` entry is appended, marked with a
  `# homevault-encrypted:{label}` comment for idempotent add/remove. `noauto`
  is deliberate — the volume must never attempt to auto-mount at boot since
  it's still locked at that point. There is **no `/etc/crypttab` entry** at
  all (crypttab would attempt auto-unlock, which contradicts "manual unlock
  only"). The pure edits (`with_marked_entry` / `without_marked_entry`) live
  in `encryption.rs` and are unit-tested; the read/`sudo tee` write is async.

## Creation is a tracked job (ROADMAP 9.1)

`POST /api/encryption/volumes` validates, starts a job and answers
`{ "success": true, "data": { "message": …, "job_id": … } }` **immediately**.
The volume does not exist yet at that point: `luksFormat` on a multi-terabyte
device plus `mkfs` plus the first `mount` take minutes, and holding an HTTP
request open for that is how a proxy timeout used to leave a half-created
container behind.

The client follows `/api/progress/{job_id}` (SSE) or
`/api/progress/{job_id}/status` (polling). The job type is
`Custom("encryption_create")`. Milestones: format (5 %) → open (30 %) → mkfs
(45 %) → mount (70 %) → UUID (80 %) → fstab (88 %) → metadata (95 %).

The SSE stream **replays the job's last known event before waiting on the
channel** and closes when that event is terminal. Without it, a job that failed
immediately — a refused `sudo`, a missing `cryptsetup`, an invalid device —
would have emitted its terminal event before the modal managed to subscribe,
and the progress panel would sit on its bar forever.

Validation stays **synchronous**, so the modal fails loud before any
privileged command runs: passphrase policy, filesystem allow-list, label,
duplicate label, then the device gate. `create` and `delete` and the
passphrase rotation are serialized behind one `VOLUME_LOCK`, so two requests
cannot both settle on the same free device.

**A failed job rolls itself back.** Once `luksFormat` has written a header,
every later failure unmounts, closes the mapper and erases the container this
job just created — so the device returns to the available pool instead of
sitting there as "in use" with no metadata row and no way to retry. A rollback
step that fails is **named** in the error, never swallowed by `let _ =`.

## Deletion is gated (ROADMAP 9.2)

`DELETE /api/encryption/volumes/{id}` requires `{ "confirm": true }` and
erases the LUKS key slots, which makes the data permanently unrecoverable
even if the disk is never wiped. Because that is irreversible, nothing is
erased until the volume is **really released**:

1. `umount` must succeed (a failure returns 400 and stops there);
2. the mapper is closed if `cryptsetup status` says it is open;
3. the state is **re-verified** from the kernel's own view —
   `/proc/self/mounts` for the mount (`erase_gate`) and `cryptsetup status`
   for the mapper — never from the exit status of the commands above;
4. only then does `cryptsetup erase` run.

The previous code was `let _ = unmount(); let _ = close(); erase();`: a busy
mount made `umount` fail, `erase` succeeded anyway, and the key material was
gone while the filesystem was still mounted and being written to. Every
refusal keeps the metadata row, so the volume stays listed and can be fixed
by hand.

After the key slots are destroyed, `wipefs -a` clears the LUKS signature so
the device returns to the available pool. If that step fails the volume is
still deleted (the data is already unrecoverable) but the response carries
`data.warnings[]` — the device keeps showing as in use until it is wiped by
hand. A failed fstab cleanup is reported the same way.

## Unlock rolls back (ROADMAP 9.3)

`POST /api/encryption/volumes/{id}/unlock` opens the container and mounts it.
If `open` succeeds and `mount` fails, the mapper is **closed again** and the
error says so. The previous code left the mapper open while the UI still
reported "locked", so every retry failed with "already mapped" and there was
no button to fix it. When even the rollback fails, the message names the
mapper and the `sudo cryptsetup close` command to run.

Lock (`POST …/lock`) reports the unmount failure instead of discarding it:
closing a mapper whose mount point is still busy cannot work, and claiming
"locked" while the filesystem is still mounted is what made the old delete
path destroy key material under a live mount.

## Passphrase policy and rotation (ROADMAP 9)

**Policy** (`encryption::validate_passphrase`), applied to **new** passphrases
only — creating a volume and choosing a new one when rotating:

| Rule | Refusal |
| --- | --- |
| At least `MIN_PASSPHRASE_LEN` = **12** characters | "at least 12 characters" |
| Not a wordlist entry (`password1234`, `123456789012`, …) and not the volume label | "too common" |
| At least four distinct characters | "too little variety" |

It is deliberately **not** applied when unlocking: a volume created under the
old 8-character rule must stay usable. The UI enforces the same 12-character
floor before sending.

**Rotation** — `POST /api/encryption/volumes/{id}/passphrase` with
`{ "old_passphrase": …, "new_passphrase": … }` — is the operation that did not
exist at all: a user who suspected a leak had to reach for a shell. It runs
`cryptsetup luksChangeKey` and then **proves the new passphrase opens the
container** with `cryptsetup open --test-passphrase`, so a silent failure can
never leave the owner locked out. It works on a locked or an unlocked volume
(a key slot is edited, not the mapper) and refuses to reuse the current
passphrase. On failure the answer says the current passphrase is unchanged.

`luksChangeKey` prompts for three lines — the current passphrase, the new one,
and a confirmation — and `change_key_stdin` builds exactly that. Sending only
two lines (the old shape) would have failed on "Passphrases do not match",
which is part of why the function had no caller.

## Bounded subprocesses

Every privileged call goes through `run_timed`, which sets `kill_on_drop(true)`
so a deadline really kills the child — a plain `Command::output()` future
dropped on timeout leaves the tool running and holding the device.

| Command | Deadline |
| --- | --- |
| `cryptsetup luksFormat` | 900 s |
| `cryptsetup open` / `luksChangeKey` | 180 s |
| `cryptsetup close` | 120 s |
| `cryptsetup erase` / `wipefs` | 600 s |
| `cryptsetup status` | 15 s |
| `cryptsetup luksUUID` | 30 s |
| `mount` / `umount` / `mkdir` | 120 s |
| `sudo tee /etc/fstab` | 60 s |

The list endpoint probes each volume's live status **concurrently** and each
probe is bounded, so a page load no longer pays the full latency of every
`cryptsetup status` one after another.

## Frontend

- `frontend/src/routes/Storage.svelte` — "Encryption" tab: lists volumes with a
  locked/unlocked badge, an inline passphrase prompt + Unlock button for
  locked volumes, Lock / Change passphrase / Delete for every volume, and an
  "Encrypt a disk" modal (device picker from the available-devices endpoint,
  filesystem choice, passphrase + confirmation, with a one-time
  non-recoverable-if-lost warning). The modal turns into a **progress panel**
  (`data-testid="storage-enc-progress"`, `role="status"`) while the create job
  runs, and the passphrase-rotation modal
  (`data-testid="storage-enc-rotate-modal"`) asks for the current passphrase,
  the new one and its confirmation.
- `frontend/src/lib/services/api.ts`: `getEncryptedVolumes`,
  `getEncryptionAvailableDevices`, `createEncryptedVolume` (answers `job_id`),
  `unlockEncryptedVolume`, `lockEncryptedVolume`,
  `changeEncryptedVolumePassphrase`, `deleteEncryptedVolume`.
- `src/i18n/{en,fr,de,es}.toml` — `[encryption]` section.

## Invariants

- Only devices where `get_available_devices()` reports `in_use == false` can be
  encrypted — the same guard the RAID page relies on, so this feature can
  never touch a disk that's part of an existing RAID array, pool, or mounted
  filesystem.
- `zfs` is rejected as a target filesystem (`ext4`/`xfs`/`btrfs` only) — ZFS
  should use its own native encryption instead.
- Labels are `[A-Za-z0-9_-]` only: the label is the mount point's last
  component and part of the fstab marker, so a second volume with the same
  label is refused rather than allowed to fight over one mount point.
- Privileged: `cryptsetup`/`mkfs.*`/`mount` all require the server process to
  run with root privileges (or equivalent capabilities) — the exact same
  requirement the existing RAID/`mdadm` code already has. If the server is run
  as an unprivileged user, both RAID and Encryption operations will fail with
  a permission error from the underlying tool, not from HomeVault's own code.
  In the generated sudoers policy each of those is scoped (ROADMAP 7.1):
  `cryptsetup` per subcommand (`luksFormat`, `open`, `close`, `status`,
  `luksUUID`, `luksChangeKey`, `erase`), `mkfs.ext4 -F`/`mkfs.xfs -f`/
  `mkfs.btrfs -f`, `wipefs -a`, `mount`/`umount` confined to `/mnt/*`, and the
  fstab entry through `tee /etc/fstab` only.

## Tests

- `src/encryption.rs` (unit) — passphrase policy, the erasure gate, the
  `/proc/self/mounts` parsing (including `\040` escapes and prefix collisions),
  the fstab pure edits, and the two properties of `run_timed` (it returns a
  fast command's output; it kills a hung one).
- `src/handlers/encryption.rs` (unit) — label/filesystem validators, the
  derived mount point, and both branches of the unlock rollback message.
- `tests/encryption_integration.rs` — the auth split on every route
  (including rotation), the passphrase policy, the duplicate-label gate,
  `DELETE` refusing to erase a volume it could not release while the metadata
  row survives, and the list reporting the live (locked) state.
- `tests/e2e/storage/storage.spec.ts` — validation-only rejection cases, plus
  the opt-in (`STORAGE_E2E_LOOP=1`) create → verify → rotate → delete
  lifecycle on a throwaway loop device, which follows the job and then proves
  the old passphrase no longer unlocks.

## Related

- [Storage](./storage.md), [RAID](./raid.md), [API Contract](../API.md)
