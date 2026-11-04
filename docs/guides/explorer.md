# File Explorer

**Purpose:** Browse, search, upload, and manage files under allowed roots. Path
access is restricted by `PathGuard`. **Editable since 2026-09-12** —
`CLAUDE.md`'s frozen-area registry is gone, so `Explorer.svelte`,
`src/handlers/explorer.rs`, `src/browser.rs`, the listed `api.ts` methods and
`[explorer]` i18n are open to edits; the invariants below still bind.

## Backend

Handlers in `src/handlers/explorer.rs`. Every handler checks
`X-Session-Token` against `state.auth.get_session(...)` inline (not the
shared `require_auth`/`require_admin` helpers, but functionally equivalent —
a real session is always required). Paths are validated against
`state.path_guard` (`AppState.path_guard`, `src/config/explorer_config.rs` /
`PathGuard`) — admins bypass the **root confinement** (they can browse
anywhere) but not the **shape normalisation**: the admin branch still
canonicalises the parent and resolves the leaf, so a handler never works on the
caller's raw string (ROADMAP 10.5, see *Invariants*); non-admins are confined
to the configured `allowed_roots`.

| Endpoint | Method | Body / Query | Notes |
|----------|--------|--------------|-------|
| `/explorer/list` | GET | `?path=` | directory listing |
| `/explorer/search` | GET | `?path=&query=&max_results=&fuzzy=` | recursive search, wired into the UI's search box (see below). `?fuzzy=true` switches to subsequence matching (see *Fuzzy search*) |
| `/explorer/disks` | GET | — | mounted volumes grouped `system\|internal\|raid\|external\|cloud\|network`; each entry carries `mount_point`, `filesystem`, `size_bytes`, `used_bytes`, plus optional `label` (FS label) + `vendor`/`model`/`serial` of the parent block device (one `lsblk -P` pass, `collect_block_device_meta`, added 2026-09-09 for the folder picker's sidebar). `FolderPicker.svelte` ignores the extra fields. |
| `/explorer/mkdir` | POST | `?path=` | create folder |
| `/explorer/delete` | POST | `?path=` | **admin-only** permanent delete — internal use only now (purge/empty-trash), the UI's Delete button calls `/explorer/trash` instead |
| `/explorer/rename` | POST | `?path=&new_name=` | rename — **both** the source and the destination (a full path) are path-guard validated, so a rename can't move a file out of the allowed roots |
| `/explorer/copy` | POST | `?source=&destination=` | copy; for a **directory** source, starts a tracked background job instead of blocking (see Advanced pool/job pattern below). A single file answers `method` (`clone` \| `kernel_copy` \| `stream`), `instant` and `bytes` — see *Instant copy (reflink)* |
| `/explorer/move` | POST | `?source=&destination=` | move; for a directory, tries an instant atomic rename first, only uses a tracked job as a cross-filesystem fallback. A same-filesystem file move answers `method: null` (an atomic rename copies nothing, so no method is claimed) |
| `/explorer/instant-copy` | GET / PUT | `{ mode }` (PUT) | instant-copy policy (see *Instant copy (reflink)*); GET any signed-in, PUT admin |
| `/explorer/permissions` | GET/POST | `?path=&mode=` | read/set POSIX mode |
| `/explorer/preview` | GET | `?path=` | text/image preview; `previewable: false` (no `content`) for anything that isn't text-like or an image |
| `/explorer/extract` | POST | `?path=&destination=` | extract archive; checks the required tool (`tar`/`unzip`/`7z`/`gunzip`) is installed first and returns a clear error instead of a raw exec failure |
| `/explorer/compress` | POST (JSON body) | `{ paths: string[], destination, format? }` | create an archive from one or more items sharing the same parent dir; `format` is `"zip"` (only if a `zip` binary is detected) or the default `"tar.gz"` |
| `/explorer/download` | GET | `?path=&ticket=` | raw file (stream); HTTP `Range` support (206) for resumable downloads / video seeking; `X-Content-Sha256` digest header for files ≤ 64 MB; rejected with 429 before the stream starts if the caller's [transfer quota](./transfer-quotas.md) is exhausted. Auth is either the `x-session-token` header or a short-lived **download ticket** (never the session token in a URL) |
| `/explorer/download-ticket` | POST | `?path=` | mint a single-use, 60-second download ticket (checks the path *and* the caller's download quota up-front, so an over-quota download surfaces as an error before the browser starts it) |
| `/explorer/thumbnail` | GET | `?path=&ticket=` | image thumbnail; auth via `x-session-token` header or a multi-use **thumbnail ticket** scoped to the file's directory |
| `/explorer/thumbnail-ticket` | POST | `?path=` | mint a multi-use, 5-minute thumbnail ticket for a directory listing |
| `/explorer/upload` | POST | multipart `file` + `?path=` | upload (XHR for progress); each chunk is checked against the caller's [transfer quota](./transfer-quotas.md) mid-write, not just once at the start |
| `/explorer/upload/session` | POST | `?path=&filename=&size=&strategy=` | open a resumable upload session (creates the hidden temp slot, validates path + filename + quota up-front) |
| `/explorer/upload/session` | DELETE | `?session_id=` | abort a resumable upload and remove its partial temp file |
| `/explorer/upload/chunk` | POST | **streamed** body + `?session_id=&offset=` | write one chunk at the server's authoritative offset; the body is written frame by frame (never buffered — ROADMAP 10.6), capped at `MAX_UPLOAD_CHUNK_BYTES` (32 MiB) and at the session's remaining `total_size`; a chunk over either cap is `413` with the offset to resume at and commits nothing; an out-of-order chunk is a 409 carrying the current offset |
| `/explorer/upload/complete` | POST | `?session_id=` | fsync + SHA-256 the temp file, resolve the collision strategy, atomically rename it into place and debit the quota (re-checked under the quota lock) |
| `/explorer/trash` | POST / GET / DELETE | `?path=` / — / — | move to trash / list everything in trash / empty trash |
| `/explorer/trash/restore` | POST | `?id=` | move a trashed item back to its original path (cross-device EXDEV fallback) |
| `/explorer/trash/{id}` | DELETE | — | permanently delete one trashed item |
| `/explorer/trash/policy` | GET / PUT | `{ retention_days, max_bytes }` (PUT) | trash retention policy (see *Trash retention & auto-purge*); GET any signed-in, PUT admin |
| `/explorer/trash/purge-now` | POST (admin) | — | run the retention pass now with the saved policy |
| `/explorer/roots` | GET | — | labelled sidebar roots: `[{path, kind, name}]` — session user's home, each PathGuard root (`kind` = users/shares/media/appdata/system/storage), then every mounted virtual disk (`kind:"virtualdisk"`) |
| `/explorer/save` | POST | `{ path, content }` | inline text/code editor — atomic temp-file + rename, 5 MB cap |
| `/explorer/snapshots` | GET | `?path=` | ZFS snapshots covering that path, each with a read-only `browse_path` under `.zfs/snapshot/` |
| `/explorer/eject` | POST (admin) | `?path=<mount>` | safe-eject a removable drive — guarded by `is_removable_block_device` (can never `umount` an internal disk) |
| `/explorer/recents` | GET / POST / DELETE | `?path=<key>` (POST) | the session user's recently-visited folders: list (newest first), touch (record/refresh one), clear (all). Stored server-side, per user, in `recent_repo` (`explorer_recents`, migration 039) — not `localStorage` |
| `/explorer/tags` | GET / POST / DELETE | `?path=` / `{ path, tag }` / `{ path, tag }` | private file/folder tags per user; list can be scoped to one path |
| `/explorer/history` | GET | `?path=` | **Rewind** — the ZFS snapshot timeline covering that file/folder, oldest→newest, each with a read-only `snapshot_path` under `.zfs/snapshot/`. Empty outside ZFS |
| `/explorer/network-discovery` | GET (admin) | — | **SMB/network discovery** — nearby shares found via `smbtree` (SMB workgroups/hosts/shares) and Avahi/mDNS (`_smb._tcp` etc.), returned as a flat `NetworkResource[]` (`kind`, `name`, `host`, `address`, `share?`). Best-effort: missing tools yield an empty list, never an error |
| `/explorer/format` | POST (admin) | `{ device, fstype, label? }` | format a block device as `ext4` or `exfat` (`mkfs.*` under `sudo`). `fstype` is validated against the two-value allow-list and the device against the available-disk list **before** any `mkfs` runs |

`/copy`, `/move`, `/upload` also take `?strategy=overwrite\|keep_both\|skip`
(`browser::CollisionStrategy`); `keep_both` uses `non_colliding_path` (` (N)`
before the extension). Upload reports `skipped[]`.

## Share by link (2026-09-12, controls 2026-10-31)

Besides the token, a link can carry a **password**, a **download cap** and —
through that cap — be a **one-time link**. The Explorer can mint a revocable
download link to a **file** and share it (URL + QR code) outside HomeVault —
the arozos `file_share.html` pattern. Backend:
`src/handlers/share_links.rs`; repo `src/storage/share_link_repo.rs` (rows in
`explorer_share_links`, **migrations 034 + 063**).

| Endpoint | Method | Body / Query | Notes |
|----------|--------|--------------|-------|
| `/explorer/shares` | GET | — | the caller's links, newest first (auth) |
| `/explorer/shares` | POST | `{ path, mode?, expires_days?, password?, max_downloads? }` | create a link for a file (auth) |
| `/explorer/shares/{token}` | DELETE | — | revoke — owner or admin (auth) |
| `/share/{token}/unlock` | POST | `{ password }` | **public** — exchanges the password for an unlock cookie |
| `/share/{token}` | GET / HEAD | — | **public** download (outside `/api`) |

Request/response shape (create/list): each link is a `ShareLinkView` —
`token`, `username`, `path`, `mode` (`anyone` \| `signed_in`), `expires_at`
(RFC-3339 or `null` = never), `created_at`, `share_path` (`/share/<token>`),
`expired`, `protected` (a password is set), `max_downloads` (`null` = no cap),
`download_count`, `exhausted` (the cap is spent). **`password_hash` and
`active_downloads` are `#[serde(skip_serializing)]`** — no response, list or
create, ever carries them.

### Password, cap and one-time: the settlement rules

- **A download counts when it finishes.** `reserve_download` atomically takes
a slot (`download_count + active_downloads < max_downloads`) *before* the first
byte, and `finish_download(completed)` releases it — incrementing
`download_count` **only** when the whole body was written. "Written" is the
**byte count** (`sent == expected`), evaluated in `poll_next` *and* in `Drop`:
hyper stops polling a body once it has written its full `Content-Length` and
drops the stream without ever observing `Poll::Ready(None)`, so keying
completion on the end-of-stream poll recorded every real download as
interrupted (and a one-time link was never spent). A failed, refused or
abandoned transfer therefore never consumes the link, and a client that
disconnects mid-stream releases its slot in `Drop`. `tests/share_links_
integration.rs::test_share_link_completed_download_counts_when_hyper_frames_
the_body` reproduces hyper's framing exactly (read `Content-Length` bytes, drop
the body) because `to_bytes` — which the other tests use — cannot catch this.
- **`max_downloads = 1` is the one-time link.** There is no separate `one_time`
column: a second mechanism would need its own arithmetic and could disagree
with the counter. The modal offers "No limit / One download / Custom".
- **Every early return after the reservation releases it** (unopenable file,
unstatable file, unsatisfiable `Range`, closed stream pool), and a `HEAD`
reserves nothing.
- **Exhaustion is part of the view**, so the owner sees `2/2` and a
"Download limit reached" badge instead of a link that silently stops working.
- **A ranged transfer counts as a download too.** A `206` for a resume or a
  video seek reserves and settles a slot like any other transfer. The
  alternative — counting only a range that covers the whole file — would let a
  client take the file in two halves and never spend the link, so the cap stays
  bypass-free at the cost of a seek costing a slot on a tightly capped link.

### The password never enters a URL

`POST /share/{token}/unlock` verifies the password once and answers with
`Set-Cookie: hv_share_unlock=<nonce>; Path=/share/<token>; HttpOnly;
SameSite=Lax[; Secure]; Max-Age=900`. The nonce lives in an in-process map
(link → nonce → expiry, 15 minutes), so a restart simply asks for the password
again, and revoking a link drops its unlock with it. The download then accepts
**either** that cookie **or** an `x-share-password` header (the API path) —
never `?password=` (it would land in the access log and the browser history).

This is what lets the public page hand the file to the browser as an ordinary
navigation: `Range`/resume work and a 40 GB file is not buffered into a
JavaScript blob first.

**Who gets the page and who gets the file.** `/share/{token}` is both the
download and the landing page, chosen by `Accept`: a browser navigation
(`text/html`) that cannot be answered with the file — a missing/incorrect
password, a spent or revoked link, a `signed_in` link — receives the SPA page,
which then probes the link (`HEAD`, `Accept: */*`) and shows the right next
step. Everything else (a download manager, `curl`, the page's own `fetch`)
keeps receiving the file or the JSON status, so the endpoint stays a
one-click download for a link that needs nothing.

A `signed_in` link accepts the `x-session-token` header **or** the `hv_session`
login cookie: a top-level navigation cannot send a header, so without the
cookie a signed-in owner could never open their own link in a tab.

**Nothing may cache a share response.** The file is served with
`Cache-Control: no-store` and `sw.js` refuses `/share/` outright — its
cache-first strategy would otherwise let a spent one-time link (or a revoked
one) replay from the browser's cache without ever touching the server. That is
not hypothetical: it is what made an E2E run report a "one-time" link that
still downloaded.

Guessing is bounded: **8 wrong passwords per link per 5 minutes**, after which
the answer is `429` — and the budget is checked *before* Argon2, because an
unthrottled password endpoint is a CPU/memory amplifier. An **empty** password
is a probe, not a guess, and costs no budget (the page uses it to decide whether
to show the field at all). A password longer than 256 characters is refused
without being hashed.

A `401` carries `x-hv-share-auth: password` or `session`, so the public page
knows whether to ask for a password or to point the visitor at sign-in.

```
POST /share/<token>/unlock  {password}
  200  Set-Cookie hv_share_unlock=…      (HttpOnly, 15 min, Path=/share/<token>)
  401  x-hv-share-auth: password
  429  too many wrong passwords (8 / 5 min)
GET  /share/<token>                      ← the cookie (or x-share-password) authorises it
  HEAD spends no slot and is honest: a spent link answers 400 before the visitor types
```

Invariants (treat as easy to break):

- **The token is the secret** — 48 random alphanumeric chars, never logged.
- Links default to **`anyone` + 30-day expiry** (secure by default); the
  caller can pick `signed_in` or `expires_days: 0` (never). Revoking a link
  cuts access immediately.
- Only **files** are shareable by link (a directory → 400); path access is
  gated by the same `PathGuard` check as every Explorer handler, so a
  non-admin can only share files inside their allowed roots.
- `signed_in` links require a valid session via the **`x-session-token`
  header only** — the session token is never accepted in the public URL's
  query string (would leak into access logs), so a bare browser tab to a
  `signed_in` link returns 401 (the public page says so and links to sign-in).
- Unknown/expired/revoked/spent tokens all answer the same "not found" error so
  the URL can't be used to probe which links ever existed; deletion only happens
  by an owner or an admin (403 otherwise).
- A path segment that is not `[A-Za-z0-9_-]{1,128}` is rejected **before** it is
  echoed into a header or a cookie `Path`.
- Create/revoke are recorded in the plain-English event log.
- The public routes are registered **outside `/api`** on their own stateful
  router and merged before the SPA fallback (`web/mod.rs::public_share`), so
  they are exempt from the CSRF layer but still pass through CORS, compression
  and the security-headers middleware.

Frontend: `frontend/src/lib/components/explorer/ShareLinkModal.svelte` (a
file-row "Share by link" action; password field, the No limit / One download /
Custom limit select, QR via the existing `qrcode` npm dep, copy, revoke, and the
user's full link list with its use count). `api.ts` gains `listShareLinks` /
`createShareLink` / `revokeShareLink` / `getShareLinkUrl`.
`frontend/src/routes/PublicShare.svelte` is the recipient's page: it `HEAD`s the
link, shows the password form only when the link has one, unlocks, then lets the
browser download. `App.svelte` renders it for any `/share/…` path, outside the
app gate and the desktop shell.

The E2E suite also cleans up links it created pointing into its `/tmp` work
dir.

## Trash (soft delete)

Deleting a file no longer removes it immediately — it moves into a
`.homevault-trash/` directory (`src/browser.rs::TRASH_DIR_NAME`) at the top
of whichever allowed root the item lives under. Each trashed item gets a
`<uuid>_<name>` on-disk name plus a `<same>.trashinfo` sidecar
(freedesktop.org Trash spec format: `Path=`/`DeletionDate=`) recording where
to restore it. `FileBrowser::trash_item`/`list_trash`/`restore_trash_item`/
`purge_trash_item`/`empty_trash` in `src/browser.rs`. The original permanent
`handle_explorer_delete` handler is unchanged and still exists — it's just
no longer what the Explorer UI's Delete button calls; purge/empty-trash use
`FileBrowser::delete_item` directly instead.

**Trash roots are canonical and shared with SMB (2026-10-29).** The two
private helpers that used to live in `explorer.rs` (`resolve_trash_root`,
`all_trash_roots`) moved to `src/trash.rs` so one place resolves every trash
folder:

- `crate::trash::root_for(allowed_roots, path)` — the trash folder for a
  given item: the top of whichever allowed root contains it; an admin
  browsing outside every configured root falls back to a single app-owned
  `base_dir()/trash` rather than scattering trash across `/etc` or `/root`.
- `crate::trash::roots_from(allowed_roots, share_paths, is_admin)` — every
  recoverable trash folder: one per allowed root, **one per configured SMB
  share** (`SharesService::smb_share_paths()` → `<share>/.homevault-trash`),
  plus the app-owned fallback for admins. The share folders are included for
  *every* user, so a non-admin still sees and restores what they deleted over
  the network. A network delete and a web delete therefore land in the same
  place and are listed, restored and cleaned up together.
- `root_for_state` / `roots_for_state` are the same helpers bound to the live
  `AppState`; `purge_all` runs the retention pass over the admin set.

## Trash retention & auto-purge (2026-10-29)

Before this, the only way a trashed file ever left the disk was the user
emptying the trash by hand — a forgotten trash with a few large videos could
quietly consume tens of gigabytes. Now an appliance-wide policy bounds it.

- **Policy** — `browser::TrashPolicy { retention_days, max_bytes }`, default
  **30 days / 10 GiB** (both `0` = that limit off). Persisted in the singleton
  `trash_policy` table (`src/storage/trash_policy_repo.rs`, migration
  **061_trash.sql**), which also stores `last_purge_at`.
- **The rules are pure and unit-tested** — `FileBrowser::select_purge(entries,
  policy, now)`: age first (every item older than `retention_days` goes), then,
  if the survivors still exceed `max_bytes`, the oldest go too until the total
  fits. An item whose `DeletionDate` cannot be parsed is never aged out but
  still counts toward the size total. `FileBrowser::purge_trash` applies the
  selection to one root and reports `{ purged, freed_bytes }`; a directory's
  size is the recursive tree size (`path_size`), so the cap counts bytes it can
  actually reclaim.
- **Background scheduler** — `src/trash_scheduler.rs` mirrors
  `journal_cleanup_scheduler`: it polls hourly and runs at most once per day.
  `plan_tick` is pure, so two rules are testable: a fully-disabled policy
  (`0/0`) does nothing at all (not even a baseline), and **`last_purge_at` only
  advances on a successful pass**, so a disk-full / permission failure is
  retried on the next tick instead of being postponed a whole cadence.
- **On demand** — an admin can run the pass now from the Trash view.

| Endpoint | Method | Auth | Notes |
|----------|--------|------|-------|
| `/explorer/trash/policy` | GET | any signed-in | current `{ retention_days, max_bytes, last_purge_at }` |
| `/explorer/trash/policy` | PUT | admin | body `{ retention_days, max_bytes }`; `0` is a real "off" value, not a reset |
| `/explorer/trash/purge-now` | POST | admin | runs the pass with the saved policy, returns `{ purged, freed_bytes }` |

Frontend: the Trash view shows an admin-only, collapsed-by-default
**"Trash retention"** panel (`data-testid="explorer-trash-retention"`) with an
age select (Forever / 7 / 30 / 90 days), a size field in GB, a Save button and
a "Clean up now" button. `api.ts`: `getTrashPolicy` / `setTrashPolicy` /
`purgeTrashNow`.

## Directory copy/move progress

A directory copy/move can take a long time, so `handle_explorer_copy`/
`handle_explorer_move` branch on `Path::is_dir()`: single files stay on the
original fast synchronous path (wrapped in `spawn_blocking`, see Invariants);
directories start a tracked background job and return `{job_id, sse_url}`
immediately, mirroring the exact `job_id`/`tokio::spawn`/`ProgressEmitter`
pattern `src/handlers/rescue.rs`'s ddrescue clone job already uses.
`FileBrowser::count_files`/`copy_dir_all_tracked` do the actual work while
`spawn_tracked_copy_job`/`spawn_tracked_move_job`/`run_tracked_copy` in
`explorer.rs` poll a shared `AtomicU64` every 300ms to emit percentage
progress. **The job must be registered via `PROGRESS_MANAGER.create_job`**
(not just a self-generated UUID) — a real bug found during live verification
had every progress event silently dropped as a no-op, because
`ProgressEmitter::progress`/`complete`/`fail` all look the job up in the
manager's map first, and a self-generated ID was never actually inserted
there. Fixed by calling `create_job` and using its returned ID.

## Transfer integrity

Every file copy/move goes through `FileBrowser::copy_file_instant`
(`src/browser.rs`; `copy_file_verified` is the same path with the reported
strategy dropped): it asks `src/browser/reflink.rs` for the cheapest strategy
that works and falls back to a streamer that hashes the source while copying
it (SHA-256), `fsync`s the destination, then reads the destination back and
compares the two hashes. A mismatch deletes the corrupt destination and returns
`BrowserError::IntegrityMismatch`, so a **move never deletes the source unless
a byte-identical copy is confirmed on disk**. `copy_dir_all`/
`copy_dir_all_tracked` recreate symlinks as symlinks (never dereference them —
avoids copying the target's contents and infinite recursion on cycles) and
preserve permission bits, ownership (uid/gid) and mtime (best-effort, like
`rsync -a`).

Every atomic `rename` (copy, move, trash, restore, upload, inline save) is
followed by a best-effort **parent-directory `fsync`**
(`FileBrowser::fsync_parent_dir`) so the new directory entry itself is durable
— without it a crash right after the rename could lose the entry even though
the file's blocks are on disk.

Uploads (`handle_explorer_upload`) `flush` + `fsync` each file before reporting
success, return each file's SHA-256 in the response `checksums[]`, and honour
an optional `X-Expected-Sha256` request header (single-file uploads): if the
stored file's hash doesn't match, the partial file is removed and the upload is
rejected.

Downloads serve files with **HTTP `Range` support** (206 + `Content-Range`, so
an interrupted download resumes and video can seek), and attach an
`X-Content-Sha256` digest header for files ≤ 64 MB so the client can verify the
transfer end-to-end. Larger files skip the digest to avoid a second full read
of a huge file purely to hash it.

**Trash** files are copied with `copy_file_verified` on the cross-filesystem
fallback too — a file is never removed from its original location unless a
byte-identical copy exists in the trash.

## Instant copy (reflink) — 2026-10-29

Copying a 40 GB file on a copy-on-write filesystem should not read and rewrite
40 GB. `src/browser/reflink.rs` gives every Explorer copy a three-step cascade,
per file, before the verified stream above is ever used:

| Step | Call | Counts as instant? |
|------|------|--------------------|
| 1 | `ioctl(FICLONE)` — the destination shares the source's extents | **yes** (`clone`) |
| 2 | `copy_file_range` (`std::io::copy`, which uses it on Linux) | no (`kernel_copy`) |
| 3 | `stream_copy_verified` — the historical hashed, read-back copy | no (`stream`) |

`copy_file_instant(src, dst, mode)` in `src/browser.rs` drives the cascade and
reports `reflink::Invocation { method, bytes }`. `CopyMethod::{Clone,
KernelClone, KernelCopy, Stream}` carries `slug()` (the wire value),
`rank()` (position in the cascade) and `is_instant()`; `KernelClone` is
**reserved** — nothing produces it today, because no syscall can prove a
kernel-side copy shared extents.

Three rules make this safe rather than merely fast:

1. **The atomic contract is unchanged.** The cascade still writes a
   `.hvtmp-<uuid>` sibling of the destination, applies the source's
   permissions/ownership/mtime *before* the swap, then `rename`s and
   `fsync`s the parent directory. A concurrent reader sees the old complete
   file or the new one, never a half-cloned file.
2. **A clone skips the read-back hash on purpose.** A reflink is
   byte-identical by construction, and hashing it would re-read the whole file
   — cancelling the entire speed-up. The verified stream is untouched and is
   still the guarantee whenever the clone and the range copy are refused.
3. **A refusal falls back; a failure does not.** `EXDEV` (another
   filesystem), `EOPNOTSUPP` (ext4, most FUSE/SMB mounts), `ENOTTY` (old
   kernel), `EINVAL` (the kernel refused this extent map — e.g. a compressed or
   encrypted file) and `EBADF` mean "this pair cannot share extents", so the
   cascade moves on. `EIO`, `ENOSPC`, `EPERM` and `EROFS` are real and are
   reported — a failing disk is never hidden behind a slow copy that happens to
   succeed. (`is_unsupported_errno` is the single place that decides.) A
   clone is therefore **never claimed** when the filesystem could not deliver
   one, which is the acceptance criterion QW-03 was written against.

### Policy and the API

The policy is a singleton row in `instant_copy_policy` (migration
**062_instant_copy.sql**, `id INTEGER PRIMARY KEY CHECK (id = 1)`), read and
written through `reflink::InstantCopyService`. `InstantCopyMode::{Auto, Off}`,
default **`auto`**; `Off` forces every copy onto the verified stream (useful
when an admin wants copies to be real, independent data — a clone shares
blocks with its source, which a sparse-snapshot policy may not want).

| Endpoint | Method | Auth | Notes |
|----------|--------|------|-------|
| `/explorer/instant-copy` | GET | any signed-in | `{ mode: "auto"\|"off", mounts: [{ mount_point, fs_type, reflink }] }` |
| `/explorer/instant-copy` | PUT | admin | body `{ mode }`; strict parse — an unknown value is a 400, never silently coerced to the default |

`instant_copy_mounts` builds the evidence through
`reflink::describe_mounts(roots, /proc/self/mounts)`: one `ReflinkMount` per
mount under the caller's roots (plus `/` when no root resolves). Btrfs is
reflink-capable by definition, XFS only with `reflink=1` in its mount options,
and **ZFS is never claimed** — pool-level block cloning is invisible to the
mount table, so the ioctl stays the authority. The list is presented as
information in the modal; it is never a gate.

`stored_mode` is lenient (`InstantCopyMode::parse` falls back to the default, so
an old or corrupt row can never break copying) while the **request** path uses
`parse_strict` — telling an admin their choice was saved when a typo was
ignored would leave the appliance on a policy they never picked. The mode is
read **once per operation** (`instant_copy_mode(&state)`) and handed to the job,
so a single copy can never mix two policies.

### What the user sees

- **Single file**: the copy reply carries `method` / `instant` / `bytes`; the
  Explorer toasts `copied_instantly` ("Copied N file(s) instantly — no data was
  duplicated") instead of the ordinary success message.
- **Directory** (tracked job): the per-file results are folded by
  `reflink::CopyStats` — one lock-free `AtomicU64` per method, incremented from
  the copy thread and summarised by the job — into
  `tracked_completion_message(stats, verb)`:
  "Copied 12 file(s) instantly (no data copied)", "Copied 12 file(s) — 9 cloned,
  3 copied", or plain "Copied 12 file(s)". A **mixed** tree is never called
  instant: the summary is the *slowest* method used (`summarize_methods`), so
  one stream copy in a hundred files is reported honestly.
- **Move**: a same-filesystem move is an atomic `rename` — better than a
  clone, and the reply says so with `method: null` rather than labelling it
  "cloned". Only the cross-filesystem fallback reports a method.

Frontend: admin-only toolbar button `explorer-instant-copy-btn` opens the
"Instant copy" modal (radio group `explorer-instant-copy-auto` /
`explorer-instant-copy-off`, the mounts list `explorer-instant-copy-mounts`,
error line `explorer-instant-copy-error`). `api.ts`: `getInstantCopySettings` /
`setInstantCopyMode`.

Two details the live run forced, both kept because they are correctness, not
polish:

- **The radios carry an explicit box** (`h-4 w-4 flex-none
  accent-[var(--hv-primary)]`). `base.css`'s form-element block sets
  `width: 100%` for text inputs and also matches `type="radio"`; without a
  size the radio stretched across its `<label>`, the option text wrapped one
  word per line, and the dialog grew to 1246 px in a 720 px viewport. See
  `docs/guides/design-system.md` §7 pitfall 6.
- **The mounts list is bounded** (`max-h-64 overflow-y-auto`) so a host with
  many filesystems scrolls the evidence instead of pushing the policy radios
  out of reach — the same class of bug, one mount table further along.

## Global file search across roots (QW-12, 2026-10-31)

The Explorer's own search is scoped to the folder you are already in
(`GET /explorer/search?path=…`). The desktop's ⌘K overlay answered "which app
or setting is called this", never "where is this file". `GET /api/search/files`
closes that gap: one query, fanned out over **every root the caller may read**,
merged, deduplicated, ranked and capped.

| Route | Auth | Answers |
|---|---|---|
| `GET /api/search/files?q=<name>&limit=<n>&files_only=<bool>` | session | `{ query, hits[], roots[], timed_out[], unreadable[], truncated, elapsed_ms }` |

`hits[]` is `FileInfo` with two extra fields: `root` (the root the hit was
found under) and `root_kind` (`home` / `users` / `shares` / `media` / `storage`
/ `virtualdisk` …), so a result row can say *where* the file lives.

### The rules, and why each one exists

- **The roots are the Explorer's, not a second list.** `handlers::search` calls
  `explorer::explorer_roots_for_session`, the exact function the sidebar uses,
  so a folder the Explorer would not offer can never produce a hit. That is one
  definition with two callers, not two lists that can drift.
- **Admins are bounded too.** The Explorer lets an admin resolve paths outside
  the configured roots; the search does **not** — it fans out over the sidebar
  roots only, so an admin's ⌘K never becomes a `find /`.
- **A query names a file, never a path.** `global_search::validate_query`
  refuses an empty or whitespace query, fewer than 2 characters (one character
  matches everything and would walk the disk on every keystroke), more than
  128 characters (counted in **characters**, not bytes, so an accented name is
  not refused), and anything containing `/`, `\` or NUL. A refusal is a `400`
  naming the rule — never a `200` with an empty list, which would read as "no
  such file".
- **Bounded, always.** Per-root budget `PER_ROOT_TIMEOUT` (1.5 s), total budget
  `TOTAL_TIMEOUT` (4 s), at most `MAX_ROOTS` (16) roots after dedupe, at most
  `MAX_RESULTS` (100) hits whatever `limit` says. A wedged network mount costs
  its own 1.5 s and is then reported in `timed_out`, never silently dropped.
- **The answer says when it is incomplete.** `timed_out` / `unreadable` name
  the roots that did not answer and `truncated` says the list was cut. The
  overlay renders that sentence under the results (`search.files_partial` /
  `search.files_truncated`, plus `search.files_unavailable` when the provider
  itself failed), because a search that hides its own gaps is worse than no
  search.
- **Dedupe and rank are pure functions.** `merge_hits` deduplicates by path
  (the same file reached through two roots appears once), ranks exact name
  before prefix before substring, then shorter names, then directories before
  files, and breaks the last tie on the path so two identical runs cannot
  reorder the list. `dedupe_roots` resolves symlinks so `/mnt/data` and
  `/mnt/data-link` are one walk.
- **Name search by default.** This provider walks names; the opt-in content
  index (D1, `docs/guides/content-index.md`) is queried beside it by the ⌘K
  overlay and merged by `fileProvider.ts`. A caller that leaves content search
  off sees exactly the name results described here.

### Frontend

`frontend/src/lib/stores/fileProvider.ts` debounces keystrokes (220 ms), drops
a superseded answer (a generation counter), and turns a hit into a row whose
action opens the hit's **folder** in the Explorer with the entry selected —
through the one-shot `explorerIntent` store, the same handoff convention
`explorerShareIntent` uses for the Shares app. The local index (apps, settings,
actions) always renders first: it answers instantly, so a file hit that had to
walk the disk never pushes "Settings" off the list.

## Download / thumbnail tickets & resumable uploads

Binary streams used to accept the session token as a `?token=` query param,
which leaks it into access logs, browser history and Referer headers. That is
gone (`src/media_ticket.rs`): a caller mints a **ticket** instead, and the
token never appears in a URL.

- **Download tickets** (`POST /explorer/download-ticket`) are single-use, tied
  to one canonical file path, valid 60 s. The frontend
  (`api.downloadFile`) mints one (the server checks path + download quota),
  then hands the ticket URL to the browser's download manager — the file
  streams straight to disk with range/resume instead of being buffered whole
  in tab memory as a blob.
- **Thumbnail tickets** (`POST /explorer/thumbnail-ticket`) are multi-use for
  5 minutes and scoped to one directory; the frontend mints one per directory
  listing and reuses it for every thumbnail `<img>`.
- **Resumable uploads** (`/upload/session` → `/upload/chunk` →
  `/upload/complete`, plus `/upload/session` DELETE to abort): a file is
  streamed in chunks and the server is authoritative about the written offset
  (an out-of-order chunk gets a 409 carrying the current offset, so the client
  resumes rather than restarting). Completion fsyncs, hashes (optional
  `?sha256=`), resolves the collision strategy and atomically renames — then
  debits the quota under the per-subject lock. The multipart `/explorer/upload`
  endpoint remains for small/simple uploads.
- **A chunk is streamed, never buffered** (2026-10-24, ROADMAP 10.6). The
  handler used to take an `axum::body::Bytes`, which materialises the whole
  chunk in RAM *before* the handler runs — with the router-wide 10 GiB
  `RequestBodyLimitLayer` as its only bound, one request could allocate the
  machine's memory. It now takes an `axum::body::Body` and writes it out frame
  by frame (`stream_chunk_to_file`), so peak memory is one network buffer
  whatever the body size. Two caps are computed **before** a byte is read: the
  declared file size (a chunk may not run past `total_size`) and
  `MAX_UPLOAD_CHUNK_BYTES` (32 MiB). Over either one → `413`, the temp file is
  truncated back to its previous length and **the session offset does not
  move**, so the client resumes from where it was.

## Directory copy/move progress

A directory copy/move can take a long time, so `handle_explorer_copy`/
`handle_explorer_move` branch on `Path::is_dir()`: single files stay on the
original fast synchronous path (wrapped in `spawn_blocking`, see Invariants);
directories start a tracked background job and return `{job_id, sse_url}`
immediately. `copy_dir_all_tracked` records every path it *created* (never one
it merely overwrote); if the job fails partway, `run_tracked_copy` removes
exactly that partial output (deepest-first) so the destination isn't left with
a misleading half-populated tree.

## Later features (2026-09-06)

- **Labelled sidebar roots** (`GET /explorer/roots`) + a "Locations" section;
  the breadcrumb shows the friendly root label + a path relative to it.
- **Inline text/code editor** in the preview modal (Edit → `<textarea>` → Save
  → `POST /explorer/save`).
- **Background auto-refresh** — originally an 8 s poll (`refreshCurrentDir`, a
  *quiet* re-list that bails while any modal/upload/search/trash/edit is
  active). The Explorer is now wired to the **live SSE watcher**
  (`GET /explorer/watch`, see "2026-10-22" below): every `change` event on the
  browsed directory triggers the same quiet re-list, so the view tracks
  server-side changes in real time instead of by polling.
- **Collision handling** — a 4-button prompt on a clash whose choice applies to
  the whole batch (`?strategy=` above).
- **File tags** — add up to 20 short labels to an accessible file or folder. Tags are private to the account, are removed when that account is deleted, and can filter ⌘K file-name results without widening the Explorer roots. Every search starts from the caller's currently visible roots; stale inaccessible tags never disclose a file. See [`file-tags.md`](file-tags.md).
- **Favorites / pinned folders** — a toolbar star, stored **per user in the
  database** (`explorer_favorites` via `src/storage/favorite_repo.rs`,
  `GET|POST|DELETE /explorer/favorites`), not in `localStorage`, so they follow
  the account across browsers (the same account-purge story as recents).
- **Safe-eject** external drives (`POST /explorer/eject`, admin).
- **"Share this folder"** — sets `$lib/stores/explorerShareIntent` and fires a
  `hv-open-app` event; `Desktop.svelte` opens the Shares window and
  `Shares.svelte` pre-fills the create form (its only Explorer hook).
- **Browse a past ZFS snapshot** — a toolbar "Snapshots" dropdown navigates into
  `.zfs/snapshot/…` and sets a **read-only** `snapshotMode`; every mutation
  bails via `blockedInSnapshot()` **except copy** (relabelled "Restore a copy
  to…" — that is how you recover an old version).

## Later features (2026-10-13)

- **Rewind (per-file snapshot timeline)** — `GET /explorer/history?path=` returns
  the ZFS snapshots that cover a path, oldest first, each carrying a read-only
  `snapshot_path` under `.zfs/snapshot/`. The toolbar "Rewind" modal lists them
  and restores by **copying out of the snapshot** (the same read-only
  `snapshotMode` copy path as the Snapshots dropdown) — there is no dedicated
  restore endpoint. Outside ZFS the list is empty and the modal says so.
- **Server-side recents** — the sidebar's "Recent" list is now stored per user
  in the database (`explorer_recents`, migration `039_explorer_recents.sql`,
  `src/storage/recent_repo.rs`), not in `localStorage`, so it follows the account
  across browsers. `GET /explorer/recents` lists them newest-first,
  `POST /explorer/recents` (body `{ path }`) records/refreshes one (called
  `recordRecent` on every folder navigation), `DELETE /explorer/recents` clears
  the list. Deleting a user purges their rows (`src/handlers/users.rs`).
- **Fuzzy search** — `?fuzzy=true` on `/explorer/search` switches from substring
  matching to a subsequence scorer (`browser::fuzzy_score`, `search_fuzzy` /
  `search_fuzzy_sync`): `rdm` matches `readme.md`, with contiguous and
  shorter-name matches ranked higher. The search box has a "fuzzy" toggle.
- **Marquee (rubber-band) select** — dragging on the file area draws a selection
  rectangle and selects the rows it crosses. Mouse enhancement only: rows stay
  keyboard-operable buttons and shift/ctrl-click is unchanged.
- **Virtualized listing** — the list/grid renders in pages (`VIRTUAL_PAGE` = 200,
  `visibleLimit` / `visibleEntries`); an `IntersectionObserver` on a sentinel
  grows the window as it scrolls, and a "show more" button is the keyboard path.
  Keeps the DOM small on huge directories.
- **Internal drag & drop** — rows are `draggable`; dropping onto a folder row
  moves the selection there (`onRowDragStart`/`onFolderDrop`), alongside the
  existing OS-file drop-to-upload.
- **SMB / network discovery** — `GET /explorer/network-discovery` (admin) lists
  nearby shares discovered with `smbtree` (SMB workgroups/hosts/shares) and
  Avahi/mDNS (`_smb._tcp`). Both parsers are pure functions unit-tested in
  `src/handlers/explorer.rs`; a host without the tools gets an empty list, not an
  error. The toolbar "Network" modal renders them as browsable entries.
  Package note: `smbtree` comes from `samba-common-bin` and `avahi-browse` from
  `avahi-utils` (not `avahi-daemon`, which only publishes HomeVault) — both are
  installed by `installer/install.sh` and verified in its Step 4.
- **Format a drive** — `POST /explorer/format` (admin) formats a block device as
  **ext4** or **exfat**. The `fstype` is checked against a two-value allow-list
  and the device against the available-disk list *before* `mkfs` runs (never a
  free-form command), then `mkfs.ext4` / `mkfs.exfat` is invoked under `sudo`.
  Package note: `mkfs.exfat` is **not** part of `exfat-fuse` — it comes from
  `exfatprogs` (Debian 12+/Ubuntu 23.04+) or `exfat-utils` (older releases);
  `installer/install.sh` installs whichever the host's repositories carry and
  verifies it in Step 4.

## Later features (2026-10-22) — security hardening (ROADMAP item 10)

This pass closed the four blocking File Explorer findings.

- **Extraction is zip-slip-safe** (`POST /explorer/extract`). Members are
  listed and validated *before* extraction: `tar -tf` / `unzip -Z1` /
  `7z l -slt` names (in the C locale — a French locale renders hard links as
  "lien vers…" instead of ` -> `) are rejected if absolute, drive-rooted, or
  containing a `..` component on either separator; archives containing **symlink
  members** are refused outright (the listings do not expose link targets, so a
  symlink cannot be pre-verified). Extraction then runs into a fresh
  `.hvextract-<uuid>` staging directory inside the destination, the staged tree
  is **verified confined** (a canonical walk resolves every symlink and refuses
  anything that resolves below the destination), and only then is the tree moved
  into the destination with the historical overwrite semantics (directories
  merge, files replace). A failure at any stage removes the staging tree.
- **The `.gz` branch streams to disk.** It used to buffer the whole
  decompressed file in RAM (`gunzip -c` stdout capture); `gunzip`'s stdout is
  now redirected straight into the output file, and a partial decompression
  (non-zero exit) is deleted.
- **The SSE watcher is authorised and can't leak** (`GET /explorer/watch`).
  `POST /explorer/watch/ticket` takes a body `{ path }`, validates the path
  through `validate_path_access` against the *calling user's* roots, and mints
  a ticket bound to **(user, path)** (TTL 15 s, single-use — the old ticket was
  bound to nothing and could watch any `?path=` the caller typed). The SSE
  endpoint redeems the ticket only for exactly that path and returns the owning
  user; events are filtered with a **component-based** `Path::starts_with`
  (a plain string prefix would leak changes inside `/media/raid2` into a watch
  on `/media/raid`). A `WatchGuard` (RAII `unwatch_path`) lives inside the SSE
  stream, so disconnects release the inotify watch instead of accumulating
  until `max_user_watches` was exhausted.
- **Resumable upload sessions are owned** (`/upload/session`, `/upload/chunk`,
  `/upload/complete`, DELETE `/upload/session`). The caller's username is
  compared to the session owner on every one — a mismatch is 403 — so a user
  who guesses a `session_id` can no longer append to, finalise, or abort
  someone else's upload. A **per-user cap** (20 concurrent slots) plus a global
  ceiling (256) refuse the 21st slot with 429; the TTL prune already kept stale
  state from accumulating.
- **Admin delete walks the PathGuard** (`POST /explorer/delete`): even for
  admins, the path is confined to the allowed roots (previously the raw query
  string reached `rm -rf`, so `?path=/` was an authenticated recursive delete),
  and deleting a **browsing root itself** is refused ("this is a browsing root,
  not an item") — a confirmed click on such a path is either a UI bug or a
  forged call and the consequence (emptying a whole volume) is not
  "delete one item". **Since 2026-10-30** one narrow exemption exists: a path
  *outside* the roots is deletable when it lives in the appliance's scratch
  space (`$TMPDIR`, `/tmp`, `/var/tmp`, checked by `is_under_temp_root` after
  `canonicalize`). Without it the admin surface was asymmetric — `mkdir`,
  upload, copy and move accept any path for an admin, so a folder created
  outside the roots (the E2E suite's `/tmp/hv-fx-e2e-*`, a harness
  `tempdir()`) could be created but never removed. Everything else outside the
  roots is still refused, and the browsing-root guard runs on whichever branch
  selected the path, so it always wins.
- **Archiving can't be argv-injected**: `FileBrowser::compress_items` passes
  `--` before the member names, so a file literally named `-o` or `-czf` is
  treated as a file to compress, never as a tool flag.
- **Every failed action is visible.** The Explorer used to swallow failure
  branches silently (`else {}`); mkdir, rename, download, extract, chmod,
  compress, upload, copy/move, trash restore/purge/empty and preview now all
  report their failure via `dlg.notify` (server message or a translated
  fallback), including *partial* batch failures ("Moved 2 of 5 items to the
  trash — the rest failed"), and the resource fetch error path is also
  reported.
- **A copy's strategy is surfaced, not just applied** (2026-10-29). The
  toolbar's admin-only Instant copy button opens a modal that explains the
  trade-off, shows which mounts can share data, and lets the admin choose
  "Instant when possible" or "Always make a full copy". Copying a file whose
  strategy was instant toasts `explorer.copied_instantly` ("Copied %count%
  file(s) instantly — no data was duplicated") — the benefit is invisible
  otherwise, since there is no progress bar to watch.

## Blocking work and deadlines (2026-10-24, ROADMAP X.1/X.2)

The Explorer was the last module still reaching the async runtime with blocking
work, and the last one whose host commands had no deadline.

- **Every handler is off the runtime now.** The disk scan
  (`GET /explorer/disks`: `/proc/mounts`, a `stat` per mount, one `lsblk` pass,
  one `df -B1` per mount) moved into `collect_disks()` behind
  `spawn_blocking`; the existence/type stats of download, upload, copy, move,
  save, roots, thumbnail and ticket paths go through `tokio::fs`
  (`path_exists` / `path_is_file` / `path_is_dir`); `resolve_dest` — which
  stats the destination and, for `keep_both`, probes for a free name — runs on
  the blocking pool; so do the durability fsyncs
  (`FileBrowser::fsync_parent_dir`); the upload-session prune unlinks with
  `tokio::fs`; and the `upload_complete` "skipped" unlink no longer blocks the
  worker. The tracked copy/move cleanup of a failed run (`cleanup_created_paths`)
  moved too, because that job runs on the async runtime.
- **Every subprocess carries a deadline** (`crate::utils::process::run_timed`):
  `lsblk` and `df` (`HOST_PROBE_TIMEOUT`, 10 s), `chmod` (10 s),
  `tar`/`unzip`/`7z`/`gunzip` including the listing passes (`ARCHIVE_TIMEOUT`,
  30 min — a ceiling for a legitimate first extraction, not an estimate),
  `sudo umount` + `udisksctl power-off` (`UMOUNT_TIMEOUT`, 30 s),
  `sudo zfs list` (`ZFS_QUERY_TIMEOUT`, 30 s), `wipefs`/`mkfs.ext4`/
  `mkfs.exfat` (`DISK_OPERATION_TIMEOUT`, 10 min) and the neighbour sweep
  (`NEIGHBOUR_TIMEOUT`, 20 s). `FileBrowser::compress_items` and its `df -T`
  probe were bounded in the same pass, so nothing reachable from this screen
  can wait forever.
- **`gunzip` writes into its file, not through this process**:
  `run_timed_to_file` hands the child a file as stdout (a `.gz` extraction used
  to capture the whole decompressed archive in RAM).
- **Two source guards keep it that way** — X.1/X.2 cannot be asserted
  behaviourally, because a blocking call still answers `200` on an idle box:
  `test_no_blocking_work_is_reachable_from_an_async_body` walks this file and
  fails on any blocking token inside an `async fn` but outside a
  `spawn_blocking` closure; `test_every_subprocess_of_this_module_carries_a_deadline`
  fails on any `.output()` / `.spawn()` / `.status()` in production code.
  `src/browser.rs` carries the subprocess half of the guard.
- **The integration net**:
  `test_disk_scan_does_not_block_the_runtime` puts a shim `lsblk` (sleeping
  2 s) first on `PATH`, starts `GET /explorer/disks`, and requires a second
  request to be served **before** it returns — on the single-threaded test
  runtime the old shape could not serve anything until the scan came back.

## Frontend

- `frontend/src/routes/Explorer.svelte` — fully wired through `$t()`/
  `translate()`.
- `frontend/src/lib/components/explorer/ExplorerWidget.svelte` — the dashboard
  widget that embeds a compact Explorer (roots + recent folders) and opens the
  full app on demand.
- Destination picking for copy/move/extract uses the shared
  `frontend/src/lib/components/theme/FolderPicker.svelte` (also now fully
  translated) instead of a raw `prompt()`. The picker overlay is
  `position: fixed`, so it **portals to `<body>`** (`use:portal`) and traps
  focus (`use:focusTrap`) — inside the Explorer's transformed /
  `overflow: hidden` window the 640px modal was clipped and its close button
  was covered by `.hv-docker-monitor`.
- Drag-and-drop upload onto the file list, in addition to the existing
  button → hidden `<input type="file">` flow (both call the same
  `uploadFiles`/`api.uploadFile`).
- **Standardized rail + status footer (2026-09-13)** — the sidebar is built
  from the shared `WindowSidebarItem`/`WindowSidebarSection` components
  (keeping the `.disk-btn` / `.disk-name` test hooks), with an **icon-rail
  collapse** toggle (`sidebarCollapseable` + `bind:sidebarCollapsed` on
  `PageLayout`) that narrows the rail to 64px instead of hiding it. The
  bottom status bar uses `WindowFooter mode="status"` (folder summary, total
  size, truncated warning + live selection count) instead of a hand-rolled
  footer inside the scroll area.
- **Sidebar drop destinations (2026-09-13)** — a sidebar root, favorite or
  mounted disk accepts an OS file drag-and-drop (the same `uploadFilesTo`
  path as the file-list drop zone) to upload straight into that location;
  the hovered row shows an inset `is-drop-active` ring.
- `frontend/src/lib/services/api.ts`: `listDirectory`, `searchFiles`,
  `getDisks`, `getExplorerRoots`, `mkdir`, `deleteItem`, `renameItem`,
  `copyItem`/`moveItem`/`uploadFile` (all take an optional `strategy`),
  `saveFile`, `getPermissions`, `setPermissions`, `previewFile`,
  `extractArchive`, `compressItems`, `trashItem`, `getTrash`,
  `restoreTrashItem`, `purgeTrashItem`, `emptyTrash`, `getTrashPolicy`,
  `setTrashPolicy`, `purgeTrashNow`, `issueDownloadTicket`,
  `getThumbnailTicket`, `downloadFile`, `getThumbnailUrl`,
  `uploadSessionStart`/`uploadChunk`/`uploadComplete`/`uploadSessionAbort`
  (chunked upload, used by `uploadFile`),
  `getExplorerSnapshots`, `ejectDrive`, `getExplorerHistory`,
  `getExplorerRecents`/`touchExplorerRecent`/`clearExplorerRecents`,
  `discoverNetworkResources`, `formatDrive`, `getInstantCopySettings`,
  `setInstantCopyMode`.
  Directory copy/move responses are checked for `data.job_id` and, if
  present, subscribed to via the existing `api.subscribeToProgress` (already
  used by Restic/Rescue, previously unused by Explorer).
- Upload/download URLs are built from `api.baseUrl` + `/explorer/...`.

## Invariants

- Every path is checked by `PathGuard`; attempts outside allowed roots are
  rejected server-side. Don't rely on the client to enforce this.
- **Handlers use the canonical path returned by `PathGuard::validate` for the
  actual filesystem work, never the raw input.** Validating a path and then
  operating on the un-canonicalised string is a TOCTOU gap (a symlink swapped
  between the check and the open can smuggle the operation out of the allowed
  roots). `validate_path_access` returns `Option<PathBuf>` for this reason.
- **The final component is resolved, not echoed back** (2026-10-24, ROADMAP
  10.5). `PathGuard::validate` used to return the caller's string verbatim when
  the leaf did not exist yet (a new file), so `..` survived and the leaf was
  whatever was typed; the admin branch of `validate_path_access` returned
  `PathBuf::from(path)` for *every* path, so nothing was always resolved there.
  Both now go through `resolve_final_component` / `resolve_unconfined`: the
  parent is canonicalised, the leaf is re-read with `symlink_metadata` (a link
  inside the roots resolves to its target, a link outside is refused), and the
  result is the resolved parent joined with a definite leaf. A `.`/`..` leaf is
  refused outright. What remains is the writer's concern — `O_NOFOLLOW` on the
  final `open`, an atomic `rename` into place — which the delete/rename paths
  already apply.
- **A public share stream never `unwrap`s its concurrency permit** (2026-10-24,
  ROADMAP 10.7). `SHARE_SEMAPHORE.acquire_owned().await.unwrap()` would panic
  into a bare 500 if the semaphore were ever closed; it now answers `503` with
  a log line. The ceiling itself (`MAX_CONCURRENT_SHARE_STREAMS` = 4) is
  unchanged — the fix is the error mapping, not the removal of the limit.
- `rename` validates **both** the source and the destination (the destination
  is a full path and was previously never checked — a non-admin could rename a
  file into `/etc`); `favorites` add is path-guarded too.
- `/explorer/delete` is **admin-only**: it is the irreversible bypass of the
  trash system. Non-admins get 403; the UI never calls it (it trashes), and
  purge/empty-trash drive `FileBrowser::delete_item` server-side. Even for
  admins the path passes the PathGuard (delete stays confined, unlike the
  create/transfer paths), with the scratch-area exemption documented above and
  in "2026-10-22"; deleting a browsing root itself is refused.
- **Extraction never trusts member names.** Listing + validation precede every
  extraction; symlink members are refused; the staged tree is verified confined
  before it is merged into the destination. Never extract a member an
  integration test would call evil (`../`, absolute, drive-root).
- **Watcher tickets are bound to (user, path).** Mint one only after
  `validate_path_access` succeeds; the SSE endpoint accepts it for that path
  only, and the inotify reference is released by `WatchGuard` on stream drop.
- **Upload sessions belong to their owner**; chunk/complete/abort compare the
  caller to `session.username` and the resumable-slot map is capped per user.
- The session token is **never** accepted in a URL. Downloads and thumbnails
  are authenticated by short-lived tickets (`src/media_ticket.rs`); the
  `?token=` query param is gone.
- **No handler blocks the runtime, and no subprocess runs without a
  deadline.** Synchronous filesystem work (including `Path::exists`/`is_dir`,
  `resolve_dest` and the fsyncs) goes through `tokio::fs` or a sync helper
  called via `tokio::task::spawn_blocking`; every `Command` goes through
  `run_timed` / `run_timed_to_file`. This is enforced by two source guards (see
  "Blocking work and deadlines") — a real bug found in live testing had these
  blocking a Tokio worker directly inside the `async fn`, and none of the 64
  integration tests caught it because a wedged disk still answers `200` slowly.
- **Trash roots are resolved in exactly one place (`src/trash.rs`), and include
  the SMB shares' recycle folders.** Never re-add a private `resolve_trash_root`
  /`all_trash_roots` in `explorer.rs`; a network delete and a web delete must
  land in the same `.homevault-trash`, or the two paths diverge and a file lost
  over the network becomes invisible in the Trash. The share folders are part of
  every user's set (a non-admin recovers what they deleted over the network).
- **The retention policy is a pure rule plus a scheduler, not a per-request
  scan.** Age and size selection live in `FileBrowser::select_purge` (unit
  tested); the background pass runs at most daily and only advances
  `last_purge_at` **after** a successful pass, so a failure is retried instead
  of skipped. A fully-disabled policy (`0/0`) removes nothing and records
  nothing. `0` on either limit means "that limit off" — never treat it as a
  reset to the default.
- **A copy reports the strategy it really used, never the strategy it was
  allowed to use.** `CopyMethod` comes from the cascade's actual outcome, so
  "cloned" is claimed only when `FICLONE` returned 0; a mixed tree folds to its
  *slowest* method (`summarize_methods`) and a same-filesystem move is
  `method: null` (an atomic `rename` copied nothing). Never hardcode
  `Clone` in a test either — the dev host's `/tmp` is ext4, so assert
  `inv.method.is_instant() == reflink::is_reflink_capable(&src)`.
- **A refused clone is not an error, a broken one is.** Only
  `EXDEV`/`EOPNOTSUPP`/`ENOTTY`/`EINVAL`/`EBADF` fall through to the range copy
  and the stream; `EIO`/`ENOSPC`/`EPERM`/`EROFS` are reported. Do not widen
  `is_unsupported_errno` to "any errno" — that would hide a failing disk behind
  a slow copy that happens to succeed.
- **The instant-copy policy is read once per operation** (`instant_copy_mode`)
  and handed to the job, so one copy can never mix two policies. The stored row
  parses leniently (an old value can never break copying) while the request
  path is strict (an admin is never told a typo was saved).
- Downloads **reserve** transfer-quota usage at stream start (under the
  per-subject quota lock) and **refund** it if the client disconnects before
  the body completes — so an abandoned download is not left charged, while a
  fully-consumed one records exactly its size.
- `FileBrowser::get_owner` resolves the numeric UID to a real username via a
  `/etc/passwd` parse (built once per call, not once per entry) — it used to
  return the raw UID as a string.
- Preview only reads/decodes file bytes for text-like mime types (`text/*`,
  `application/json`, `application/javascript`, `application/xml`); anything
  else (video, audio, PDF, archives, unknown binaries) skips the read
  entirely and reports `previewable: false` — it used to lossily decode
  arbitrary binary data as UTF-8 and render the garbled result in a
  `<pre>` block.
- Uploads use the resumable chunked flow (`/upload/session|chunk|complete`),
  reported to the caller as progress per chunk; the legacy
  `multipart/form-data` `/upload` endpoint still exists and is what the
  integration tests drive. All non-binary calls use `fetch` via `api.*`.
- Permissions set/get operates on POSIX mode strings (e.g. `0755`).
- Archive creation defaults to `tar.gz` (always available via core utils);
  `.zip` is only offered if a `zip` binary is actually detected at request
  time. The installer provisions `zip`/`unzip`/`7z` (the `7z` binary comes
  from `7zip` on Debian 12+/Ubuntu 23.04+ and `p7zip-full` on older
  releases) as one optional, non-fatal group — on a host where the admin
  declined or the repo lacked those packages, the archive actions degrade to
  a clear "not installed" error instead of a raw exec failure.
- The inline editor's close guard (2026-09-12): once the user has typed in
  the editor (`editDirty`), closing the preview (X / Escape / backdrop)
  opens a "Save / Discard / Cancel" confirm instead of silently dropping the
  edits — the preview modal closes first and reopens on "Cancel" with the
  in-progress `editContent` intact (the shared `Modal`'s internal close
  state can't fight a still-open parent).
- **Format is a two-value allow-list, never a free-form command.** `fstype` is
  rejected unless it is exactly `ext4` or `exfat`, and the device must be in the
  available-disk list, *before* any `mkfs` runs — the handler never interpolates
  caller input into a shell command.
- **Network discovery never fails the request.** A host without `smbtree` or
  Avahi returns an empty list; the parsers are pure functions with unit tests, so
  a malformed tool output degrades to "nothing found" instead of a 500.
- **Recents are per-user and server-side.** The rows are keyed by the session
  user and purged when the account is deleted; the client never sends another
  user's key.
- **Directory navigation and watcher refreshes are sequence-guarded.**
  `Explorer.svelte` tracks `loadSeq` and `watchSeq` so out-of-order
  `listDirectory` or `refreshCurrentDir` responses and stale SSE `EventSource`
  watchers from a previous folder can never overwrite the active directory
  listing. When multiple sidebar roots share the same `kind` (e.g. `/media` and
  `/mnt` both classify as `media`), `rootLabel` appends the root path so the
  sidebar and breadcrumb stay distinct.
- **Rewind restores by copy, not by writing into the snapshot.** The snapshot
  path is read-only (`.zfs/snapshot/…`); "restore" copies the old version out to
  a chosen destination, so a snapshot is never mutated.
- **Verified live** (real files, real HTTP calls, not just code review):
  owner resolution, preview gating (text vs. binary), recursive search
  finding a deeply nested file, full trash lifecycle (trash → list → restore
  → trash again → purge, confirmed byte-for-byte on disk at each step),
  compress → extract round-trip (byte-identical diff), a 400-file/77MB
  directory copy via the tracked-progress job (byte-identical diff,
  real SSE `progress`/`done` events observed), and an instant same-filesystem
  directory move. Auth was verified via `curl`: every new endpoint rejects a
  request with no session token.

## Residual hardening (2026-10-24, ROADMAP 10.5/10.6/10.7)

The three leftovers item 10 deliberately left open, all real on a normal path.

### 10.5 — the final path component is resolved

`PathGuard::validate` returned the **caller's string** when the leaf did not
exist yet ("a new file"), and `validate_path_access`'s admin branch returned
`PathBuf::from(path)` for *every* admin path. On both, the last component was
never canonicalised: `..` survived, and a symlink planted after validation was
followed with no second look.

Now both go through one shape:

- `PathGuard::resolve_final_component(parent_resolved, p)` — used by
  `PathGuard::validate` (absolute **and** relative branch);
- `explorer::resolve_unconfined(path)` — the admin branch, same shape but
  **without** root confinement (an admin may browse anywhere).

The parent is canonicalised, the leaf is re-read with
`symlink_metadata` (which does *not* follow it): a symlink that resolves inside
the roots becomes its target, a symlink that resolves outside is refused, a
regular leaf that appeared meanwhile is resolved and re-checked, and an absent
leaf comes back under its resolved parent. A `.`/`..` leaf is refused outright.

What is left is the **writer's** half, and it is already handled where it
matters: `delete`/`rename`/`create` open and rename into place rather than
resolving a name twice. This change removes the unexamined hand-over; closing
the window completely is an `O_NOFOLLOW`-on-open property, not a
path-resolution one.

### 10.6 — a chunk is streamed, never buffered

The handler took `axum::body::Bytes`: the whole chunk was materialised in RAM
before the handler ran, and the only bound was the router-wide 10 GiB
`RequestBodyLimitLayer`. One request could allocate the machine's memory.

It now takes `axum::body::Body` and writes it out through
`stream_chunk_to_file`, frame by frame, so peak memory is one network buffer.
Two caps are computed *before* a byte is read — the session's remaining
`total_size - offset` and `MAX_UPLOAD_CHUNK_BYTES` (32 MiB, public so the client
can honour it). Over either: `413` with the resume offset, the temp file
truncated back to its previous length, and **the session offset does not
move** — the resumable protocol's invariant is "the offset only advances on a
full chunk".

### 10.7 — the share-stream permit is a mapped error

`SHARE_SEMAPHORE.acquire_owned().await.unwrap()` panicked if the semaphore was
closed: a bare 500 with nothing in the request log, and a task abort in a debug
build. It cannot happen with the static (nothing calls `close`) — which is why
it went unnoticed, and exactly why an `unwrap` on a resource whose lifecycle
lives elsewhere is a latent panic. It now logs and answers `503`. The ceiling
(`MAX_CONCURRENT_SHARE_STREAMS` = 4) is unchanged.

### Tests

| Where | What |
|---|---|
| `src/config/explorer_config.rs` (4 → **9**) | a new file comes back under its resolved parent with no `..`; a symlink leaf pointing outside is refused; one inside resolves to its target; a `.`/`..` leaf is refused; the relative branch |
| `src/handlers/explorer.rs` (38 → **46**) | the admin branch normalises; it does *not* confine (`/etc/hostname` still resolves); unusable shapes refused; an existing symlink resolves; a chunk written from a 3-frame stream; an over-cap chunk rolls the file back; an empty stream writes nothing; the route cap stays below the router limit |
| `src/handlers/share_links.rs` (0 → **2**) | a closed semaphore yields an error, and the source no longer contains `acquire_owned().await.unwrap()`; the 4-stream ceiling still holds |
| `tests/explorer_integration.rs` (65 → **70**) | a streamed multi-frame chunk lands byte for byte and completes; a chunk past the declared size is `413`, does not move the offset, and the session still completes with the accepted bytes; an admin `..` path lists the resolved directory; an admin path outside the roots still works; a non-admin traversal is still `400` |

## Progress reporting (fixed 2026-10-12)

Two defects in the shared progress system this screen depends on — both are
fixed.

`ProgressManager::get_progress` (`src/sse/progress.rs`, behind the
`GET /progress/{job_id}/status` REST endpoint) used to return a hardcoded
`"Waiting..."` / 0 % / `Running` placeholder for any job present in the map
instead of its real last-known state. It now returns the job's `last_event` —
the same snapshot the SSE stream and the activity feed carry — and `None` for an
id that was never created or was already removed. Explorer's own frontend uses
the SSE stream (`api.subscribeToProgress`), not this REST endpoint, so it was
theoretical here, but the endpoint is public API and lied to any polling client
(it also covered `src/handlers/rescue.rs`'s job status polling).

**The SSE stream was *not* merely theoretical, though — a directory copy/move
needed this fix to work.** `JobType` used internal tagging
(`#[serde(tag = "type")]`) with a `Custom(String)` newtype variant, which serde
cannot serialize at all (*"cannot serialize tagged newtype variant
`JobType::Custom` containing a string"*). `explorer_copy` / `explorer_move` are
`Custom` jobs — as are Docker install, antivirus, RAG, VM, rescue, cloud sync and
Ollama pulls — so `handle_progress_sse`'s
`serde_json::to_string(&event).unwrap_or_default()` sent an **empty** `data:`
payload: `JSON.parse` throws in the browser, `onProgress` and `onDone` never
fire, the `EventSource` error handler takes over, and `watchJob` resolves
**false** for a copy that actually succeeded — frozen progress bar, stale
directory listing, `success` counter stuck at 0. (Storage's cache mover is a
built-in `CacheMover` job, so it kept working — the two SSE consumers disagreed,
which is what hid the bug.) `GET /api/progress/jobs` also panicked inside the
`json!` macro while any `Custom` job was tracked; the desktop taskbar was spared
because the activity handler maps `JobType` to a slug (`job_type_kind`) and never
serializes the enum. Fixed by **adjacent** tagging —
`#[serde(tag = "type", content = "name", rename_all = "snake_case")]`: the
built-in kinds keep their previous wire shape exactly (`{"type":"backup"}`), a
feature job becomes `{"type":"custom","name":"explorer_copy"}`. Do not
"simplify" it back to `tag` alone; the serde round-trip tests in
`src/sse/progress.rs` guard it. The Explorer directory copy/move path itself
should be re-run live to confirm the progress bar now animates and the listing
refreshes (see `TESTING.md`).

## Transfer tray, enriched progress and history — 2026-10-31

DSM's File Station dominates one dimension HomeVault was weak on: **seeing what
is happening**. It has a three-zone tray (uploads / downloads / background) with
animated badges, `"45.3 MB / 128.7 MB"` + throughput + time remaining, a
per-task cancel, and a list of finished tasks. This section is the HomeVault
answer, built on the existing `PROGRESS_MANAGER` rather than a second job
system. A comparison document (DSM vs HomeVault, file transfer) drove it; the
work is HV-UX-01/02/03, HV-INT-01 and HV-ADV-02 in that document.

### HV-UX-02 — the progress event carries the transfer detail

`ProgressEvent` (and `ActivitySnapshot`, and the `ActivityTask` wire shape) grew
four fields, all `Option` and all `null` when the producer does not know:

| Field | Meaning |
|-------|---------|
| `bytes_processed` | cumulative bytes moved (already existed on the event, now also on the activity/tray wire) |
| `bytes_total` | declared or measured total |
| `speed_bps` | bytes/second over a **rolling 5 s window** (`SpeedTracker`) |
| `eta_seconds` | seconds remaining, computed server-side from the two above |
| `method` | the copy method / digest slug (`clone`, `kernel_copy`, `stream`, `sha256`) |

Producers emit through `ProgressEmitter::transfer_progress(job_id, pct, message,
&TransferDetail { … })`; `emit_progress_extended` is kept and delegates, so
existing callers are untouched. `SpeedTracker` (in `src/sse/progress.rs`) is pure
`Instant` arithmetic — no clock reads inside it — so the unit tests drive it with
fixed instants: two samples before a rate exists, the slope over the window (not
the run-long average), pruning, a backwards counter treated as a fresh baseline
(a retried chunk), and an ETA that rounds up and answers `0` — never `None` — once
the total is reached.

The **client formats, the server reports numbers**. `frontend/src/lib/utils/
transferFormat.ts` owns `formatBytes` / `formatSpeed` / `formatEta` /
`transferDetailLine` / `progressPercent`, and the Explorer's own upload bar now
shows `45.3 MB / 128.7 MB` next to the percentage.

### HV-UX-01 — the tray (`TransferTray.svelte`)

A toolbar button (`data-testid="explorer-transfers-btn"`, badge =
`counts.total`) opens a `Modal` holding `TransferTray.svelte`. Three zone
buttons with badges, one row per transfer: name, progress bar
(`role="progressbar"`, indeterminate when the percentage is `null`), the detail
line, and a **cancel** button per row. A `<details>` section at the bottom lists
the last ten finished transfers (the audit trail, below).

The tray polls **one** endpoint, `GET /api/explorer/transfers/active`, every 2 s
(the toolbar badge polls the same endpoint every 5 s and pauses while the tray is
open). One poll is deliberate: the tray needs both the progress-manager jobs
**and** the in-flight chunked uploads, which are not jobs at all — they are rows
in `upload_sessions` (see below). The desktop taskbar's `/api/activity` stays the
appliance-wide feed; the tray is the Explorer's narrower question, and the zone
grouping (`zone_for_kind`) only the Explorer can answer.

Cancelling is real, not cosmetic — and the button only appears on a row that
can actually be stopped. An upload is aborted with
`DELETE /api/explorer/upload/session`; a job is cancelled with
`DELETE /api/progress/{id}`, which runs the producer's cancellation hook. The
server advertises `cancel_path` **only when the job registered a hook**
(`ProgressManager::has_cancel_hook`), so the tray never shows a button that
would merely hide a row. Concretely: a **checksum job** is cancellable (it owns
its hash loop, and the hook flips a flag the per-block callback reads, ending
the run without a digest), an **upload** is cancellable, and a tracked
**directory copy** is **not** — its copy loop does not observe a stop flag, so
no button is offered instead of a false promise.

**Deviation:** there is no per-task *pause/resume*. Pausing a server-side stream
means holding its body producer and its file handle across requests — a
different data path from the one these transfers use (they are synchronous
copies on the blocking pool, and the upload protocol is client-driven chunks).
Cancel + resume-from-offset already covers the user-visible need for uploads
(the client resends from the server's offset), so pause was not faked.

### HV-UX-03 — resumable uploads survive a restart

`upload_sessions` (migration **074**) is the durable half of the chunked upload
state; the process-wide map stays as a write-through cache because the chunk
path is hot. Writes are ordered database-first, so the cache is never ahead of
the row:

| Event | Effect |
|-------|--------|
| `POST /upload/session` | row inserted (`offset = 0`, `last_chunk_at = now`, `speed_bps = NULL`); a failed insert refuses the slot rather than handing out an id only this process knows |
| `POST /upload/chunk` | cache hit, else **load the row and rehydrate**; `offset` + `last_chunk_at` + `speed_bps` (last chunk's throughput) updated in one statement |
| `POST /upload/complete` | row deleted after the atomic rename |
| `DELETE /upload/session` | row deleted with the temp file |
| any session endpoint | `prune_expired` deletes idle rows (`last_chunk_at` older than the 30-minute TTL) and unlinks each temp file |

Three consequences worth knowing:

1. **The TTL is idle-based**, measured from `last_chunk_at`, not from creation:
   a slow multi-gigabyte upload that keeps receiving chunks never expires
   mid-flight, and an abandoned one is swept with its temp file.
2. **The concurrency caps count rows**, not map entries (256 total, 20 per
   user). A cap that only looked at the map let one account start twenty more
   uploads on top of the twenty a restart had orphaned.
3. **A row whose temp file disappeared is dropped**, not answered: the next chunk
   cannot be appended to a file that is not there, and a silent `200` would only
   move the failure to `complete`.

`src/upload_session.rs` owns the SQL; `StoredUploadSession::progress` /
`eta_seconds` / `is_expired` are pure and unit-tested.

### HV-INT-01 — on-demand checksums

DSM ships a whole module for MD5 (`SYNO.FileStation.MD5`). HomeVault now offers
the same capability with a modern digest:

| Endpoint | Method | Auth | Notes |
|----------|--------|------|-------|
| `/explorer/checksum` | POST | auth | body `{ path, algorithm?, compare_with?, async? }`; `sha256` (default) \| `sha512` \| `blake3`; both paths are PathGuard-validated |
| `/explorer/checksum/result` | GET | auth | `?job_id=` — the outcome of an asynchronous run (owner or admin only; the owner is stored with the outcome, so the check survives the job leaving the progress registry) |

Behaviour:

- A file up to `checksum::INLINE_MAX_BYTES` (8 MiB) is hashed **inside the
  request** (`spawn_blocking`, 64 KiB blocks) and answers
  `{ path, algorithm, checksum, file_size, duration_ms }`.
- A larger file — or any file with `"async": true` — becomes a tracked job
  (`JobType::Custom("explorer_checksum")`) answering `{ job_id, sse_url,
  status: "running" }`. The job publishes bytes, speed and ETA through
  `SpeedTracker`, so it looks like every other transfer in the tray, and its
  digest is read back from `/checksum/result` (a `JobOutcomeStore`, bounded to
  256 entries and 15 minutes — a progress event is not a place to keep a result).
- `compare_with` hashes a second file with the same algorithm and answers
  `{ compare_with: { path, checksum, file_size }, match: bool }` — the "prove the
  copy is identical" case that otherwise sends a user to a shell.
- **MD5 is refused**, and so is any unknown algorithm: a typo is a `400`, never a
  silent fallback to SHA-256.

BLAKE3 comes from the `blake3` crate; the module holds known-vector tests for all
three algorithms (`""`, `"abc"`), a cumulative-progress test, the
`NotAFile`/`Io` error paths and `digests_match`'s case/whitespace tolerance.

### HV-ADV-02 — transfer history

`transfer_history` (migration **073**) records one row per finished transfer:
`job_id`, `username`, `operation` (`upload`/`download`/`copy`/`move`/`checksum`),
`source_path`, `dest_path`, `bytes_total`, `duration_ms`, `method`, `verified`,
`status` (`complete`/`failed`/`cancelled`), `error_message`, `created_at`,
`completed_at` (unix seconds).

- **Written best-effort**: `record_transfer` logs and swallows a database error.
  History is bookkeeping; a failed insert must never turn a successful upload
  into an error the user cannot fix (the bytes are already on disk).
- **Downloads are recorded when the body ends, not when it starts.**
  `QuotaRefundStream` already knew whether the stream reached its end (it refunds
  the quota reservation otherwise); it now also writes `complete` or `cancelled`
  from that same signal, because a handler returns long before the last byte
  leaves.
- **`verified` is honest**: `true` only when the bytes were proven identical (an
  upload whose `X-Expected-Sha256`/session digest matched, a copy that took the
  verified stream, a checksum whose `compare_with` matched). A clone is byte
  identical *by construction* and is recorded with `verified = false`, not with a
  re-hash it never performed.
- **Retention is 90 days**, pruned at most hourly (`should_prune`) on the write
  path.

| Endpoint | Method | Auth | Notes |
|----------|--------|------|-------|
| `/explorer/transfers/active` | GET | auth | `{ uploads: [...], transfers: [...], counts: { uploads, downloads, copies, total } }` |
| `/explorer/transfers/history` | GET | auth | `?limit=&offset=&operation=&status=&since=`; a non-admin sees only their own rows, an admin sees every user's; unknown filter values are a `400`; `has_more` comes from a page fetched one row deep (no second COUNT) |

A completed job never appears in the *live* list (the screen that started it
reports its own success); a failed or cancelled one stays for 60 s
(`FAILED_GRACE_SECS`) so the row the user was watching reports its outcome.

### RAG

`src/rag_indexers.rs`'s `explorer` `FeatureDoc` lists the tray, the detailed
progress, the resumable-upload durability, the checksum endpoint and the history,
so the assistant can answer "how do I verify a copy?" from the same facts as this
guide.

## Related

- [Storage](./storage.md), [Shares](./shares.md), [API Contract](../API.md)
