# Disk Encryption (LUKS2)

**Purpose:** Encrypt a single available (unassigned) block device with LUKS2.
Deliberately scoped to standalone devices — not RAID arrays (those stay on the
[RAID](./raid.md) page) and not ZFS pools (ZFS already has native
dataset encryption, a separate mechanism). Unlock is always **manual**: no
key material is ever stored on the server, so a volume stays locked after
every reboot until an admin re-enters the passphrase through the UI.

## Backend

- `src/encryption.rs` — shells out to `cryptsetup` (`luksFormat`, `open`,
  `close`, `status`, `luksUUID`, `luksChangeKey`, `erase`) plus `mount`/`umount`.
  Passphrases are always piped via **stdin** (`--key-file -`), never passed as a
  CLI argument, so they never appear in the process list.
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
  only"). This marker-append/remove logic is implemented locally in
  `encryption.rs` rather than reusing `disk.rs`'s `append_marked_entry` /
  `remove_marked_entry` — those belong to the RAID mark format.

## Frontend

- `frontend/src/routes/Storage.svelte` — new "Encryption" tab: lists volumes
  with a locked/unlocked badge, an inline passphrase prompt + Unlock button for
  locked volumes, Lock/Delete for unlocked ones, and an "Encrypt a disk" modal
  (device picker from the available-devices endpoint, filesystem choice,
  passphrase + confirmation, with a one-time non-recoverable-if-lost warning).
- `frontend/src/lib/services/api.ts`: `getEncryptedVolumes`,
  `getEncryptionAvailableDevices`, `createEncryptedVolume`,
  `unlockEncryptedVolume`, `lockEncryptedVolume`, `deleteEncryptedVolume`.
- `src/i18n/{en,fr,de,es}.toml` — `[encryption]` section.

## Invariants

- Only devices where `get_available_devices()` reports `in_use == false` can be
  encrypted — the same guard the RAID page relies on, so this feature can
  never touch a disk that's part of an existing RAID array, pool, or mounted
  filesystem.
- `zfs` is rejected as a target filesystem (`ext4`/`xfs`/`btrfs` only) — ZFS
  should use its own native encryption instead.
- Deleting a volume (`DELETE /encryption/volumes/{id}`) requires
  `{ "confirm": true }` and runs `cryptsetup erase`, which destroys the LUKS
  key slots — the underlying data becomes permanently unrecoverable even if
  the disk itself isn't wiped.
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

## Related

- [Storage](./storage.md), [RAID](./raid.md), [API Contract](../API.md)
