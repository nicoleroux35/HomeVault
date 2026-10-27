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
| `/explorer/copy` | POST | `?source=&destination=` | copy; for a **directory** source, starts a tracked background job instead of blocking (see Advanced pool/job pattern below) |
| `/explorer/move` | POST | `?source=&destination=` | move; for a directory, tries an instant atomic rename first, only uses a tracked job as a cross-filesystem fallback |
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
| `/explorer/roots` | GET | — | labelled sidebar roots: `[{path, kind, name}]` — session user's home, each PathGuard root (`kind` = users/shares/media/appdata/system/storage), then every mounted virtual disk (`kind:"virtualdisk"`) |
| `/explorer/save` | POST | `{ path, content }` | inline text/code editor — atomic temp-file + rename, 5 MB cap |
| `/explorer/snapshots` | GET | `?path=` | ZFS snapshots covering that path, each with a read-only `browse_path` under `.zfs/snapshot/` |
| `/explorer/eject` | POST (admin) | `?path=<mount>` | safe-eject a removable drive — guarded by `is_removable_block_device` (can never `umount` an internal disk) |
| `/explorer/recents` | GET / POST / DELETE | `?path=<key>` (POST) | the session user's recently-visited folders: list (newest first), touch (record/refresh one), clear (all). Stored server-side, per user, in `recent_repo` (`explorer_recents`, migration 039) — not `localStorage` |
| `/explorer/history` | GET | `?path=` | **Rewind** — the ZFS snapshot timeline covering that file/folder, oldest→newest, each with a read-only `snapshot_path` under `.zfs/snapshot/`. Empty outside ZFS |
| `/explorer/network-discovery` | GET (admin) | — | **SMB/network discovery** — nearby shares found via `smbtree` (SMB workgroups/hosts/shares) and Avahi/mDNS (`_smb._tcp` etc.), returned as a flat `NetworkResource[]` (`kind`, `name`, `host`, `address`, `share?`). Best-effort: missing tools yield an empty list, never an error |
| `/explorer/format` | POST (admin) | `{ device, fstype, label? }` | format a block device as `ext4` or `exfat` (`mkfs.*` under `sudo`). `fstype` is validated against the two-value allow-list and the device against the available-disk list **before** any `mkfs` runs |

`/copy`, `/move`, `/upload` also take `?strategy=overwrite\|keep_both\|skip`
(`browser::CollisionStrategy`); `keep_both` uses `non_colliding_path` (` (N)`
before the extension). Upload reports `skipped[]`.

## Share by link (2026-09-12)

The Explorer can mint a revocable download link to a **file** and share it
(URL + QR code) outside HomeVault — the arozos `file_share.html` pattern.
Backend: `src/handlers/share_links.rs`; repo `src/storage/share_link_repo.rs`
(rows in `explorer_share_links`, **migration 034**).

| Endpoint | Method | Body / Query | Notes |
|----------|--------|--------------|-------|
| `/explorer/shares` | GET | — | the caller's links, newest first (auth) |
| `/explorer/shares` | POST | `{ path, mode?, expires_days? }` | create a link for a file (auth) |
| `/explorer/shares/{token}` | DELETE | — | revoke — owner or admin (auth) |
| `/share/{token}` | GET | — | **public** download (outside `/api`) |

Request/response shape (create/list): each link is a `ShareLinkView` —
`token`, `username`, `path`, `mode` (`anyone` \| `signed_in`), `expires_at`
(RFC-3339 or `null` = never), `created_at`, `share_path` (`/share/<token>`),
`expired` (bool).

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
  `signed_in` link returns 401.
- Unknown/expired/revoked tokens all answer the same "not found" error so the
  URL can't be used to probe which links ever existed; deletion only happens
  by an owner or an admin (403 otherwise).
- Create/revoke are recorded in the plain-English event log.
- The public route is registered **outside `/api`** on its own stateful
  router and merged before the SPA fallback (`web/mod.rs::public_share`), so
  it is exempt from the CSRF layer (GET-only) but still passes through CORS,
  compression and the security-headers middleware.

Frontend: `frontend/src/lib/components/explorer/ShareLinkModal.svelte` (a
file-row "Share by link" action; QR via the existing `qrcode` npm dep, copy,
revoke, and the user's full link list as a manager). `api.ts` gains
`listShareLinks` / `createShareLink` / `revokeShareLink` / `getShareLinkUrl`.
The E2E suite also cleans up links it created pointing into its `/tmp` work
dir.

## Trash (soft delete)

Deleting a file no longer removes it immediately — it moves into a
`.homevault-trash/` directory (`src/browser.rs::TRASH_DIR_NAME`) at the top
of whichever allowed root the item lives under (resolved by
`resolve_trash_root` in `explorer.rs`; admins working outside any configured
root fall back to a single app-owned `base_dir()/trash`, rather than
scattering trash folders across arbitrary system directories). Each trashed
item gets a `<uuid>_<name>` on-disk name plus a `<same>.trashinfo` sidecar
(freedesktop.org Trash spec format: `Path=`/`DeletionDate=`) recording where
to restore it. `FileBrowser::trash_item`/`list_trash`/`restore_trash_item`/
`purge_trash_item`/`empty_trash` in `src/browser.rs`. The original permanent
`handle_explorer_delete` handler is unchanged and still exists — it's just
no longer what the Explorer UI's Delete button calls; purge/empty-trash use
`FileBrowser::delete_item` directly instead.

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

Every file copy/move goes through `FileBrowser::copy_file_verified`
(`src/browser.rs`): it streams the source while hashing it (SHA-256),
`fsync`s the destination, then reads the destination back and compares the two
hashes. A mismatch deletes the corrupt destination and returns
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
  "delete one item".
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
  `restoreTrashItem`, `purgeTrashItem`, `emptyTrash`, `issueDownloadTicket`,
  `getThumbnailTicket`, `downloadFile`, `getThumbnailUrl`,
  `uploadSessionStart`/`uploadChunk`/`uploadComplete`/`uploadSessionAbort`
  (chunked upload, used by `uploadFile`),
  `getExplorerSnapshots`, `ejectDrive`, `getExplorerHistory`,
  `getExplorerRecents`/`touchExplorerRecent`/`clearExplorerRecents`,
  `discoverNetworkResources`, `formatDrive`.
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
  admins the path passes the PathGuard (delete is confined like every other
  mutation), and deleting a browsing root itself is refused (see "2026-10-22").
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

## Related

- [Storage](./storage.md), [Shares](./shares.md), [API Contract](../API.md)
