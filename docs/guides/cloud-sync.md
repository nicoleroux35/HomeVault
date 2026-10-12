# Cloud Sync (rclone)

**Purpose:** Manage rclone remotes, mounts, and sync/copy jobs to cloud storage.

## Backend

Handlers in `src/handlers/rclone.rs`. Every handler takes `headers: HeaderMap`
and checks a real session before doing anything — local `check_session`/
`check_admin` helpers (`Result<String, String>`, since these handlers return
`Json<ApiResponse<T>>` rather than `axum::response::Response`, so the shared
`admin_guard::require_auth`/`require_admin` couldn't be reused directly
without a large mechanical rewrite of every return point).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /rclone/status` | auth | rclone available? |
| `GET /rclone/remotes` | auth | configured remotes |
| `GET /rclone/providers` | auth | supported providers |
| `GET /rclone/mounts` | auth | active mounts |
| `POST /rclone/remotes` | admin | add remote |
| `DELETE /rclone/remotes/{name}` | admin | remove remote |
| `POST /rclone/mount/start\|stop` | admin | mount/unmount |
| `POST /rclone/sync\|copy` | admin | transfer jobs |
| `GET /rclone/config` | admin | raw config — exposes every remote's credentials, same sensitivity bar as Restic's `?include_password=true` |
| `POST /rclone/config` | admin | save raw config |
| `GET /rclone/jobs` | auth | list persisted sync jobs |
| `POST /rclone/jobs` | admin | create a persisted sync job |
| `DELETE /rclone/jobs/{id}` | admin | delete a sync job |
| `POST /rclone/jobs/{id}/run` | admin | one-shot "Run now" — reuses the `execute_sync` core extracted from `handle_sync`; there is **no scheduler**, jobs never auto-run |

`POST /rclone/crypt` was removed entirely — it was a fake stub (always
returned a canned "Crypt operation configured" string without doing
anything) with zero frontend callers, confirmed via a repo-wide grep before
deleting it.

Sync jobs are persisted server-side (`sqlx/migrations/036_sync_jobs.sql`,
table `sync_jobs`); the frontend used to keep them in `localStorage` (rows
vanished on another browser/profile) and now reads/writes them through the API
above. Read-only listing requires a session; create/delete/run are admin
mutations (`check_session`/`check_admin` in `src/storage/sync_job_repo.rs` +
`src/handlers/rclone.rs`).

## Frontend

- `frontend/src/routes/CloudSync.svelte` — fully wired through `$t()`/
  `translate()` (previously had zero i18n calls anywhere in the file,
  ignoring the selected language entirely, the same bug class already found
  on Dashboard/Network/Shares/Chat/Explorer).
- Local source/destination path picking uses the shared
  `frontend/src/lib/components/theme/FolderPicker.svelte` (already
  integrated into Explorer) — a real filesystem browser instead of the
  previous "Local" mode misapplying the cloud-remote-name `<select>` as a
  filesystem path. The remote `<select>` is still used, unchanged, for the
  `cloud` type.
- `frontend/src/lib/services/api.ts`: `getRcloneStatus`, `getCloudRemotes`,
  `getCloudProviders`, `getCloudMounts`, `addCloudRemote`, `deleteCloudRemote`,
  `mountCloudRemote`, `unmountCloudRemote`, `syncCloud`, `copyCloud`,
  `getCloudConfig`, `saveCloudConfig`, `listSyncJobs`, `createSyncJob`,
  `deleteSyncJob`, `runSyncJob`. `mountCloudRemote(remote, mountPoint,
  path)` only ever sends `{ remote, mount_point, path }` — see the mount fix
  below.

## Confirmed bugs found and fixed

1. **No auth anywhere.** Every handler took only `State(_state)`, no session
   check at all — `GET /rclone/config` returned the raw `/etc/rclone.conf`
   file to anyone who could reach the server. rclone "obscures" secrets with
   a trivially-reversible encoding (not real encryption), so this was a
   plaintext credential leak for every configured remote (S3 keys, OAuth
   tokens, FTP/SFTP passwords). `sync`/`copy` accept a `"local"` path type
   with no `PathGuard`-style restriction, so combined with the missing auth
   this was also an unauthenticated arbitrary-file-read-and-write primitive
   (e.g. sync `local:/etc/shadow` out to a self-added remote). Fixed: reads
   use `check_session`, mutations use `check_admin`.
2. **The mount feature was completely broken.** `MountRequest`'s
   `read_only`/`allow_other`/`daemon` fields had no `#[serde(default)]`, but
   the frontend's `api.mountCloudRemote()` only ever sends
   `{ remote, mount_point, path }` — every real "Start Mount" click failed
   Axum's JSON deserialization before the handler even ran. Separately, the
   old foreground (non-`daemon`) code path awaited `rclone mount`'s own
   process output, but `rclone mount` without `--daemon` runs in the
   foreground indefinitely — once the deserialization bug was fixed, that
   branch would have hung the HTTP request forever. Fixed by adding
   `#[serde(default)]` to the three bool fields and removing the foreground
   branch entirely — `handle_mount_start` now always spawns non-blocking
   with `--daemon`.
3. **`handle_add_remote` config-injection.** `req.config` (free text) was
   appended verbatim into the INI-style `/etc/rclone.conf` with no
   validation — a value containing an embedded `\n[othersection]` line could
   silently inject or override an unrelated, already-configured remote's
   section. Fixed: any `config` value with a line starting with `[` (after
   trim) is now rejected with a clear error.
4. **Dead `handle_crypt` stub** — removed (see above).
5. **Misapplied remote-name dropdown for local paths** in
   `CloudSync.svelte` — replaced with `FolderPicker` (see Frontend above).

## Invariants

- Read endpoints require a valid session (`check_session`); anything that
  can start a transfer, mount a filesystem, or read/write the raw config
  requires an admin session (`check_admin`).
- `add_remote` rejects any `config` value containing a line starting with
  `[`, preventing config-section injection.
- `handle_mount_start` always spawns rclone non-blocking (`cmd.spawn()`,
  `--daemon`) — never awaits the mount process's own output.
- **One config path, resolved in one place.** `rclone_config_path()`
  (`src/handlers/rclone.rs`) is the **only** way this module obtains the file it
  reads *and* writes: an explicit non-empty `RCLONE_CONFIG` wins outright
  (existing or not — that is how a config is created outside `/etc`), else the
  first of rclone's locations that exists (`/etc/rclone.conf`,
  `/root/.config/rclone/rclone.conf`, `$HOME/.config/rclone/rclone.conf`), else
  `/etc/rclone.conf`. `parse_rclone_config()` goes through it too, instead of
  looping over the candidates on its own. **Do not reintroduce a hardcoded
  `/etc/rclone.conf` in a handler**: `handle_get_config`, `_save_config`,
  `_add_remote` and `_delete_remote` all used to, so the Settings editor read
  and wrote a different file than the one the remote list was parsed from — an
  admin who moved the config (the supported way to run rclone as a non-root
  user) got an empty list beside unrelated editor contents, and saving failed
  with `Permission denied` (against a root-owned `/etc`) for anyone but root.
  Guarded by `handlers::rclone::tests::test_parse_and_the_handlers_resolve_the_
  same_file` and, over HTTP, by
  `tests/rclone_integration.rs::config_endpoints_use_the_rclone_config_override`,
  `save_config_writes_the_overridden_file_and_keeps_a_backup` and
  `add_list_delete_remote_round_trip_on_the_overridden_file`.
- `handle_sync` / `handle_copy` register an **indeterminate** `cloud_sync`
  `PROGRESS_MANAGER` job around the blocking `rclone` call so the transfer
  shows in the desktop [activity taskbar](./activity.md); completed/failed on
  every return branch, response bodies unchanged. (Determinate `--stats`
  parsing would need converting the handler to `spawn` + piped stderr — a
  follow-up.)
- **The config path is no longer hardcoded anywhere** — see the invariant
  below (`rclone_config_path()`). The two halves used to disagree: the parser
  honoured `RCLONE_CONFIG`, the four read/write handlers did not.
- **Verified live** (isolated dev server, a scratch `RCLONE_CONFIG` pointing
  at a fake `[testremote] type = local` config, real `curl` calls, not
  mocks): every endpoint now rejects a request with no session token;
  `add_remote` rejects a `config` value containing an embedded
  `[othersection]` line; a real `rclone mount --daemon` was started via the
  API using only `{remote, mount_point, path}` — the exact shape the real
  frontend sends — and confirmed as a genuine, active FUSE mount via
  `mount | grep` and `mountpoint`, not just a 200 response; the mount was
  then stopped via the API and confirmed unmounted. `POST /rclone/crypt`
  now falls through to the SPA catch-all (route genuinely gone).

## Related

- [Explorer](./explorer.md), [Docker](./docker.md), [API Contract](../API.md)
