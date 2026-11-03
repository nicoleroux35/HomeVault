# ISO / IMG mounting (read-only)

**Purpose:** browse the contents of a disc image — an installer ISO, a `.img`
backup, a virtual-machine appliance — without extracting it or copying it
anywhere. The image is attached to a loop device read-only and appears in the
Explorer as one more folder, so a 6 GB ISO opens instantly instead of becoming
6 GB of files on the array.

**Who can use it:** any signed-in account may read what is mounted. Mounting and
unmounting are **admin-only** — both run root-owned `losetup`/`mount`/`umount`,
and a mount point is a directory the whole appliance can then browse.

## Where it lives

- **Screen:** Explorer sidebar → **Disc images** →
  `frontend/src/lib/components/explorer/IsoMountPanel.svelte`.
- **Backend:** `src/iso_mount.rs` (validation, slug, containment, argv
  generation, `/proc/mounts` parsing — all pure) and
  `src/handlers/iso_mount.rs` (HTTP surface, the only part that shells out).
- **Storage:** `{DATA_DIRECTORY}/iso/<slug>/` (the mount point) and
  `{DATA_DIRECTORY}/iso/mounts.json` (the label/loop-device index).
- **Dependencies:** `losetup` (util-linux), `mount`/`umount` (util-linux).
  When any is missing the panel says so and offers nothing, per
  `docs/API.md` → capability contract.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET` | `/api/explorer/iso` | `require_auth` — `{ available, iso_root, mounts[] }` |
| `POST` | `/api/explorer/iso` | `require_admin` — `{ path }` → the mounted row |
| `DELETE` | `/api/explorer/iso/{id}` | `require_admin` |

## Rules

- **Read-only, `nosuid`, `nodev`.** `mount -o loop,ro,nosuid,nodev` — always.
  A writable mount of an image that happens to hold a writable filesystem lets
  a browse operation mutate the source archive, and a crafted image could
  otherwise introduce a setuid binary or a device node into the appliance's
  namespace. `mount_is_read_only_nosuid_and_nodev` asserts all four flags.
- **The mount point is derived, never requested.** It is
  `<iso_root>/<slug>`, where `iso_root` is `{DATA_DIRECTORY}/iso` and the slug
  is `<readable-name>-<16 hex digits of the full path>`. A client cannot name a
  directory. The hash is what lets `ubuntu.iso` in two folders mount side by
  side instead of one silently replacing the other, and being deterministic
  means re-mounting reuses the same directory instead of leaking one per
  attempt.
- **The image must already be readable by the caller.** The allowed roots come
  from `explorer::explorer_roots_for_session` — the exact function the Explorer
  sidebar uses — so a folder the sidebar would not show can never be mounted,
  and an admin's mount is bounded the same way the ⌘K search is.
- **A `..` is refused before `canonicalize`.** A lexical path like
  `/data/../etc/foo.iso` would pass a containment test on the string and then
  resolve elsewhere in the kernel, so the traversal is rejected outright first.
- **The extension is an allowlist** (`.iso`, `.img`). `mount -o loop` will treat
  many things as a filesystem; only a disc image goes through this path, and the
  refusal names the accepted set.
- **Mounting the same image twice is refused**, naming the first mount rather
  than stacking a second loop device on it.
- **The filesystem is the authority.** `mounts.json` only carries the label and
  the loop device of a mount `/proc/mounts` already confirms; a directory with
  no mount on it is never reported as mounted. A corrupt index degrades to "no
  known labels", not to an error.
- **A failed mount does not leak the loop device.** If `mount` refuses the image
  (not a filesystem, corrupt partition table), the handler detaches the loop
  device it attached and removes the directory before returning the error.
- **Unmounting re-derives and re-checks the mount point** from the id, so a
  crafted id cannot name a path. A detach failure after a successful `umount`
  is not reported as a failed unmount — the mount is genuinely gone.

## Status codes

Per `docs/API.md`:

| Situation | Status |
|---|---|
| Image outside the caller's roots, wrong extension, missing, `..` in the path | `400` — the request is refused on its merits |
| `losetup`/`mount` not installed | `503` — a capability this host lacks |
| No mount with that id | `400`, naming the id |
| Malformed body with a session | `400`, naming the field |
| No session / not an admin | `401` / `403` |

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/iso_mount.rs` (`#[cfg(test)]`) | 20 unit — extension allowlist, slug determinism/collision/bounds, mount-point containment + `..` escape, pre-flight refusals, the four argv builders (`ro`/`nosuid`/`nodev`, no `--detach-all`), `/proc/mounts` parsing and the "not mounted ⇒ not reported" rule |
| `src/handlers/iso_mount.rs` (`#[cfg(test)]`) | 5 unit — relative path, `..` traversal, missing file, a real file inside a root, mount-point derivation |
| `tests/iso_mount_integration.rs` | 11 — auth matrix, guard-before-body (`401` not `422`), payload shape, four refusal paths, unknown id, crafted id |
| `frontend/src/lib/components/explorer/IsoMountPanel.svelte` | covered by `npm run check` |

**Not covered by an automated test:** a successful `losetup`+`mount` round
trip. The build host has no passwordless sudo and a real mount would leave a
device attached, so the argv and the containment are asserted directly instead.
That is the same deviation `docs/guides/iscsi.md` records for its hardware
suite.

## Related

- [File Explorer](./explorer.md) — where the mounted folder appears
- [Storage](./storage.md) — disks and virtual disks
- [Rescue](./rescue.md) — a different read-only view of block devices
