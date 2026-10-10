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
`PathGuard`) — admins bypass `PathGuard` entirely and can browse anywhere;
non-admins are restricted to the configured `allowed_roots`.

| Endpoint | Method | Body / Query | Notes |
|----------|--------|--------------|-------|
| `/explorer/list` | GET | `?path=` | directory listing |
| `/explorer/search` | GET | `?path=&query=&max_results=` | recursive search, wired into the UI's search box (see below) |
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
| `/explorer/upload/chunk` | POST | raw body + `?session_id=&offset=` | write one chunk at the server's authoritative offset; an out-of-order chunk is rejected with a 409 carrying the current offset so the client can resume |
| `/explorer/upload/complete` | POST | `?session_id=` | fsync + SHA-256 the temp file, resolve the collision strategy, atomically rename it into place and debit the quota (re-checked under the quota lock) |
| `/explorer/trash` | POST / GET / DELETE | `?path=` / — / — | move to trash / list everything in trash / empty trash |
| `/explorer/trash/restore` | POST | `?id=` | move a trashed item back to its original path (cross-device EXDEV fallback) |
| `/explorer/trash/{id}` | DELETE | — | permanently delete one trashed item |
| `/explorer/roots` | GET | — | labelled sidebar roots: `[{path, kind, name}]` — session user's home, each PathGuard root (`kind` = users/shares/media/appdata/system/storage), then every mounted virtual disk (`kind:"virtualdisk"`) |
| `/explorer/save` | POST | `{ path, content }` | inline text/code editor — atomic temp-file + rename, 5 MB cap |
| `/explorer/snapshots` | GET | `?path=` | ZFS snapshots covering that path, each with a read-only `browse_path` under `.zfs/snapshot/` |
| `/explorer/eject` | POST (admin) | `?path=<mount>` | safe-eject a removable drive — guarded by `is_removable_block_device` (can never `umount` an internal disk) |

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
- **Background auto-refresh** — an 8 s poll (`refreshCurrentDir`, a *quiet*
  re-list) that bails while any modal/upload/search/trash/edit is active. Not
  real inotify; a `notify`-crate → SSE watcher is the follow-up.
- **Collision handling** — a 4-button prompt on a clash whose choice applies to
  the whole batch (`?strategy=` above).
- **Favorites / pinned folders** — a toolbar star; stored **per browser in
  `localStorage`** (`hv-explorer-favorites`), not server-side.
- **Safe-eject** external drives (`POST /explorer/eject`, admin).
- **"Share this folder"** — sets `$lib/stores/explorerShareIntent` and fires a
  `hv-open-app` event; `Desktop.svelte` opens the Shares window and
  `Shares.svelte` pre-fills the create form (its only Explorer hook).
- **Browse a past ZFS snapshot** — a toolbar "Snapshots" dropdown navigates into
  `.zfs/snapshot/…` and sets a **read-only** `snapshotMode`; every mutation
  bails via `blockedInSnapshot()` **except copy** (relabelled "Restore a copy
  to…" — that is how you recover an old version).

## Frontend

- `frontend/src/routes/Explorer.svelte` — fully wired through `$t()`/
  `translate()`.
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
  `getExplorerSnapshots`, `ejectDrive`.
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
- `rename` validates **both** the source and the destination (the destination
  is a full path and was previously never checked — a non-admin could rename a
  file into `/etc`); `favorites` add is path-guarded too.
- `/explorer/delete` is **admin-only**: it is the irreversible bypass of the
  trash system. Non-admins get 403; the UI never calls it (it trashes), and
  purge/empty-trash drive `FileBrowser::delete_item` server-side.
- The session token is **never** accepted in a URL. Downloads and thumbnails
  are authenticated by short-lived tickets (`src/media_ticket.rs`); the
  `?token=` query param is gone.
- `mkdir`/`delete`/`rename`/`copy`/`move`/`extract`/`permissions`/`preview`
  handlers all wrap their synchronous filesystem/`Command` work in
  `tokio::task::spawn_blocking`, matching `list_directory`/`search`'s
  existing pattern — a real bug found live testing had these blocking a
  Tokio worker thread directly inside the `async fn` instead.
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
- **Verified live** (real files, real HTTP calls, not just code review):
  owner resolution, preview gating (text vs. binary), recursive search
  finding a deeply nested file, full trash lifecycle (trash → list → restore
  → trash again → purge, confirmed byte-for-byte on disk at each step),
  compress → extract round-trip (byte-identical diff), a 400-file/77MB
  directory copy via the tracked-progress job (byte-identical diff,
  real SSE `progress`/`done` events observed), and an instant same-filesystem
  directory move. Auth was verified via `curl`: every new endpoint rejects a
  request with no session token.

## Known pre-existing issue (not fixed here, out of scope)

`ProgressManager::get_progress` (`src/sse/progress.rs`, used by the
`GET /progress/{job_id}/status` REST endpoint, not the SSE stream) always
returns a hardcoded `"Waiting..."` / 0% / Running placeholder for any job
that exists in the map, rather than its real last-known state — it doesn't
track/store the most recent `ProgressEvent` per job. This predates Explorer's
use of the shared progress system (also affects `src/handlers/rescue.rs`'s
job status polling) and wasn't introduced or fixed here; Explorer's own
frontend uses the SSE stream (`api.subscribeToProgress`), not this REST
endpoint, so it isn't affected in practice.

## Related

- [Storage](./storage.md), [Shares](./shares.md), [API Contract](../API.md)
