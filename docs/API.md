# API Contract

All endpoints are mounted under `/api` by `src/web/mod.rs` (`create_app`). The
SPA's `ApiClient` (`frontend/src/lib/services/api.ts`) prefixes every call with
`/api`. Paths below are written **without** the `/api` prefix.

> This file is a curated map, not generated — `CLAUDE.md`'s engineering
> invariants are the current authority, with the per-feature detail in
> `guides/`. Reconciled with them **2026-09-08**.

## Conventions

- **Request**: `POST`/`PUT` bodies are JSON with `Content-Type: application/json`.
  Uploads (`/explorer/upload`, avatar upload) use `multipart/form-data`.
- **Auth headers**: every authenticated request sends `X-Session-Token`. CSRF-
  protected mutations also send `X-CSRF-Token`.
- **Response envelope**: `ApiResponse<T>` = `{ success: bool, data?, error? }`
  (or `message` on some older handlers). On failure, `error`/`message` carries a
  human-readable string the client shows in a toast.
- **Auth levels** (see `src/handlers/admin_guard.rs`):
  - `auth` — any valid session (`require_auth`).
  - `admin` — admin session (`require_admin`).
  - `public` — no session required.
- **Caching**: `ApiClient` caches successful `GET`s for a per-call TTL (default
  5 s) and invalidates cache keys on mutations.

## Auth — `auth/*`

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| POST | `/auth/login` | public | `handle_login` — 401 bad credentials, 429 per-IP throttle / account lockout; response includes `must_change_password` (admin-chosen password pending) |
| POST | `/auth/logout` | auth | `handle_logout` |
| GET | `/auth/me` | auth | `handle_get_current_user` — includes `must_change_password` |
| POST | `/auth/2fa/verify` | public | `handle_2fa_verify` |
| POST | `/auth/2fa/setup` | auth | `handle_2fa_setup` |
| POST | `/auth/recovery-code` | auth | `handle_generate_recovery_code` (400 if one exists unless `?regenerate=true`) |
| POST | `/auth/verify-recovery-code` | public | `handle_verify_recovery_code` — `{ username, code }` |
| POST | `/auth/reset-password-recovery` | public | `handle_reset_password_recovery` — code is not consumed |
| GET | `/auth/setup-status` | public | `handle_setup_status` — `is_first_run` (DB-derived), `is_setup_valid`, `hardware_changed`, `needs_recovery`, `server_machine_id`, `sudo_available`, `system_language` (host POSIX locale → `en`/`fr`/`de`/`es`) |
| GET | `/auth/check-username` | public (local network only, first-run only) | `handle_check_username` — 403 once an admin exists (post-setup it would be a free username oracle) |
| POST | `/auth/setup` | public (local network only) | `handle_setup` — accepts optional `totp_secret` + `totp_code` (enrolled at the wizard's password step); response may include `totp_enabled` + `backup_codes`. Refused once an admin exists |
| POST | `/auth/setup/verify-totp` | public (local network only, first-run only) | `handle_setup_verify_totp` — stateless pre-creation check of a client-generated TOTP secret: `{ secret, code }` → `{ valid }` |
| POST | `/auth/setup/rebaseline` | admin | `handle_setup_rebaseline` — re-baseline the hardware fingerprint after a confirmed hardware change |

The three unauthenticated bootstrap routes (`/auth/setup`,
`/auth/setup/verify-totp`, `/auth/check-username`) accept only loopback /
private / link-local peers (`middleware::setup_guard`), **only same-origin
browser contexts** (a foreign `Origin` or `Sec-Fetch-Site: cross-site` → `403`),
and are **rate-limited to 20 requests/min per peer** (`429`).
`HOMEVAULT_SETUP_ALLOW_REMOTE=1` lifts only the network check.
All three are also **first-run-only at the handler level**: once an admin
exists `/auth/setup` refuses ("Setup already complete"), and
`/auth/setup/verify-totp` + `/auth/check-username` answer `403`.
`/auth/setup/progress` sits behind the same network + same-origin guard but is
**exempt from the 20/min budget** — it is the wizard's own step bookkeeping
(one PUT per navigation), and metering it made the wizard exhaust its budget
and starve its own `check-username` calls.
`/auth/setup-status` is exempt from all of it (public by design, read by the
login screen).

Login returns a session token (stored as `session-token` in localStorage) and
sets an `hv_session` cookie read only by the app-proxy auth entry point:
`GET /app-auth?app=<store>:<app>` (browser navigation, unauthenticated by
design — it is the redirect target of a gated app's proxy). It validates the
`hv_session` cookie, mints an app-proxy-scoped `hv_app_session` and redirects
to the app's proxy port; under an HTTPS deployment the gate's redirect targets
the TLS entry, because the `Secure` session cookie would never travel over
plain HTTP (see [Apps Center](./guides/apps-center.md)).
`POST /auth/setup` returns the same session shape plus `totp_enabled` and
`backup_codes` when TOTP was enrolled at the wizard's password step.
2FA-enabled accounts: `/auth/login` returns `requires_2fa`; the TOTP code is sent
to `/auth/2fa/verify`. The old email `/auth/forgot-password` + `/auth/reset-password`
flow was **removed** 2026-09-03 — see [Authentication](./guides/auth.md).

## Settings — `settings/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/settings` | auth | aggregate of all categories |
| GET | `/settings/public` | public | safe subset (server name, language, theme) |
| GET/PUT | `/settings/general` | auth / admin | server name, timezone |
| GET/POST | `/settings/language` | auth | set UI language (persisted) |
| GET | `/translations` | auth | returns flattened `locale → key → string` map (`handle_get_translations`, `src/handlers/settings.rs`; mounted at `/api/translations`) |
| GET/PUT | `/settings/email` | auth / admin | SMTP |
| POST | `/settings/email/test` | admin | send test email |
| GET/PUT | `/settings/notifications` | auth / admin | |
| GET/PUT | `/settings/appearance` | auth / admin | theme, wallpaper, accent, density |
| GET/PUT | `/settings/update` | auth / admin | update-check frequency |
| GET | `/settings/version` | auth | current version info |
| GET | `/settings/export` | admin | JSON dump |
| POST | `/settings/import` | admin | JSON import |
| POST | `/settings/reset` | admin | reset to defaults |
| GET/PUT | `/settings/tls` | admin | built-in HTTPS config + cert info; PUT returns `requires_restart` |
| POST | `/settings/tls/regenerate` | admin | force self-signed cert regeneration |
| GET | `/settings/backup/db` | admin | snapshots of HomeVault's own database — settings + files on disk + total size (the DB is WAL, so the `.db` alone is not restorable) |
| PUT | `/settings/backup/db` | admin | update `interval_hours` / `retain`; a lower retention prunes immediately |
| POST | `/settings/backup/db/now` | admin | take a `VACUUM INTO` snapshot now → `{ backup, status }` |
| GET/PUT | `/settings/autostart` | admin | start-on-boot (systemd) for `homevault.service` |

## Security — `security/*` (auth)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/security/sessions` | auth | `handle_get_my_sessions` |
| POST | `/security/sessions/revoke` | auth | `handle_revoke_sessions` |
| POST | `/security/sessions/revoke-all` | auth | `handle_revoke_all_sessions` |
| GET | `/security/login-history` | admin | `handle_get_login_history` — truthful rows: each recorded attempt carries the real `success` flag and the HTTP `User-Agent`; rate-limited failures are kept (they are security-relevant) but the per-user counters only count `success = 0` attempts, and `clear_attempts` only purges failures — successful logins stay in history |
| GET | `/security/login-history/{username}` | admin | `handle_get_user_login_history` — same truthful success/user-agent per row |

## Dashboard & widgets

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/dashboard/data` | auth | CPU/RAM/disk/network summary |
| GET | `/dashboard/widget/{id}` | auth | data for one widget |
| POST | `/dashboard/widget` | auth | add widget |
| DELETE | `/dashboard/widget/{id}` | auth | remove widget |
| POST | `/dashboard/widgets/reorder` | auth | reorder |
| GET | `/dashboard/widgets` | auth | all widgets |
| GET | `/dashboard/network-traffic` | auth | |
| GET | `/dashboard/cpu-temperature` | auth | |
| GET | `/dashboard/storage-volumes` | auth | |
| GET/POST | `/dashboard/widget-preferences` | auth | per-user widget prefs |
| GET | `/dashboard/disk-io` | auth | |
| GET | `/dashboard/storage` | auth | |
| GET | `/dashboard/health` | auth | |
| GET | `/dashboard/server-details` | auth | motherboard/BIOS model |
| GET | `/health` | auth | overall health + CPU/mem from `/proc` |
| GET | `/health/live` | public | liveness probe — `200 {"status":"ok"}` whenever the HTTP stack responds. No host detail (public route) |
| GET | `/health/ready` | public | readiness probe — `200` ready / `503` degraded. Runs `SELECT 1` (1 s cache) and reports a failed boot-time `PRAGMA quick_check`. No host detail |
| GET | `/widgets/cpu` | auth | desktop widget |
| GET | `/widgets/ram` | auth | |
| GET | `/widgets/network-traffic` | auth | |
| GET | `/widgets/cpu-temp` | auth | |
| GET | `/widgets/update-info` | auth | real check when `UPDATE_CHECK_URL` is set (JSON `{"version":"<semver>"}`, compared with `is_newer_version`, results cached process-wide for 1 h); offline / timeout / malformed payload → `has_app_update=false` without noise |

## Server control

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| POST | `/server/restart` | admin | |
| POST | `/server/shutdown` | admin | |
| GET | `/server/info` | auth | IP/port/display address |

## Users — `users/*` (most `admin`)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/users` | admin | `handle_users_list` |
| POST | `/users` | admin | `handle_user_create` |
| GET | `/users/{username}` | admin | `handle_user_get` |
| PUT | `/users/{username}` | admin | `handle_user_update` |
| PUT | `/users/{username}/full` | admin | `handle_user_update_full` |
| DELETE | `/users/{username}` | admin | `handle_user_delete` (kills PTY sessions; revokes the account's direct share grants — ACL + DB — and purges its SFTP jail) |
| PUT | `/users/{username}/password` | admin | `handle_user_update_password` — admin-vs-target sets `must_change_password` (owner must change it at first sign-in); self change clears it. A flagged session is limited server-side (`middleware::password_change_guard`) to whoami/logout/its own password change, other calls get `403 password_change_required` |
| GET | `/users/{username}/sessions` | admin | `handle_admin_user_sessions` — active sessions of that user, tokens masked to 8 chars + `…` |
| DELETE | `/users/{username}/sessions` | admin | `handle_admin_revoke_user_sessions` — revoke all of that user's sessions (`{"revoked": n}`) |
| PUT | `/users/{username}/expiry` | admin | `handle_user_set_expiry` — set (`{"expires_at": "<RFC 3339>"}`, must be future) or clear (`null`) the horizon; enforcement in `AuthService::authenticate` + the session sweep |
| PUT | `/users/{username}/role` | admin | `handle_user_update_role` |
| PUT | `/users/{username}/locked` | admin | `handle_user_set_locked` — a **permanent** lock (clears `locked_until`); an automatic lockout instead expires on its own (see `guides/auth.md`) |
| POST | `/users/{username}/2fa/disable` | admin | `handle_admin_disable_2fa` |
| PUT | `/users/{username}/groups` | admin | `handle_user_update_groups` |
| GET | `/users/{username}/home` | admin | `handle_user_home_get` (returns `size_bytes`) |
| PUT | `/users/{username}/home` | admin | `handle_user_home_put` (`{ enabled, disk, size_bytes }`) |
| DELETE | `/users/{username}/home` | admin | `handle_user_home_delete` |
| GET | `/users/{username}/avatar.svg` | auth | `handle_user_avatar` (SVG) |
| GET | `/users/count` | admin | `handle_user_count` |
| GET | `/users/{username}/exists` | admin | `handle_user_exists` |
| POST | `/auth/me/avatar` | auth | `handle_update_my_avatar` (self) |

## Groups — `groups/*` (most `admin`)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/groups` | admin | `handle_groups_list` |
| POST | `/groups` | admin | `handle_group_create` |
| GET | `/groups/{name}` | admin | `handle_group_get` |
| PUT | `/groups/{name}` | admin | `handle_group_update` — `{ name?, description? }`: rename (migrates the mirror, group grants, quotas and ACLs) and/or set the description |
| DELETE | `/groups/{name}` | admin | `handle_group_delete` |
| POST | `/groups/{name}/members` | admin | `handle_group_add_member` |
| DELETE | `/groups/{name}/members/{username}` | admin | `handle_group_remove_member` |
| GET | `/groups/users` | admin | `handle_groups_list_users` |
| GET | `/users/{username}/groups` | admin | `handle_user_groups` |

The `homevault_admin` system group (the OS admin group) is hidden from ordinary
group management and cannot be created/deleted through these endpoints
(see `src/groups.rs` `is_admin_group`).

**Home-directory size:** `POST /users` and `PUT /users/{u}/full` also accept
`home_size_bytes: Option<i64>` (`0`/`null` = no limit; negative → 400; larger
than the disk's free space → 400). It applies a real `setquota -u` quota on the
home directory (best-effort — recorded even if the FS lacks `usrquota`). See
[Users](./guides/users.md#home-directory-size-limit-2026-09-08).

## Storage — `storage/*` (auth)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/storage/disks` | auth | `handle_disks_list` |
| GET | `/storage/removable` | auth | `handle_removable_disks_list` |
| GET | `/storage/partitions` | auth | `handle_partitions_list` (`lsblk -l`) |
| GET | `/storage/raid/devices` | auth | `handle_raid_available_devices` — candidate physical disks for the RAID creation wizard. **Every** non-system disk is listed; `inUse` is facts-based (partitions, filesystem, mount point, md/ZFS/Btrfs superblock, system disk), in-use rows carry a human `reason`, and every row reports SMART overall health (`smart_status`: `Healthy` / `Failed` / `Unknown`) |
| POST | `/storage/raid` | admin | `handle_raid_create` — creates one virtual disk **as a tracked background job**: `{message, job_id}` immediately, milestones + completion/failure on the progress SSE of `job_id`. Body: `{ name, level (raid0|raid1|raid5|raid6|raid10), devices, filesystem (ext4|xfs|btrfs|zfs), spares? (mdadm hot spares), allow_missing? (degraded create, mdadm redundant levels only) }`. Gates answer 400 synchronously: pure request-shape validation, **server-side device gate** (every device must exist, not host the system disk, and be entirely free) and name-collision pre-check |
| GET | `/storage/virtual-disks` | auth | `handle_virtual_disks_list` — **all** virtual disks (mdadm + ZFS + Btrfs) |
| DELETE | `/storage/virtual-disks/{name}` | admin | `handle_virtual_disk_delete` — dispatches mdadm vs ZFS/Btrfs |
| POST | `/storage/virtual-disks/{name}/scrub` | admin | `handle_virtual_disk_scrub` |
| GET | `/storage/virtual-disks/{name}/scrub` | auth | `handle_virtual_disk_scrub_status` — current scrub state + `in_progress` |
| GET | `/storage/importable-pools` | auth | `handle_importable_pools_list` — foreign ZFS/Btrfs pools on attached disks |
| POST | `/storage/import-pool` | admin | `handle_import_pool` — `{ name, pool_type, new_name? }`, ZFS `zpool import` / Btrfs mount |
| GET | `/storage/raid` | auth | `handle_raid_arrays_list` — legacy; Storage's Virtual-disks tab is preferred |
| DELETE | `/storage/raid/{name}` | admin | `handle_raid_delete` — legacy |
| POST | `/storage/raid/{name}/rebuild` | admin | `handle_raid_rebuild` — reused by Storage's Rebuild button; the optional `replacement_device` passes the same free-device gate as creation |
| POST | `/storage/virtual-disks/{name}/grow` | admin | `handle_virtual_disk_grow` — mdadm only: `{ device }` (a free whole disk) → `mdadm --add` + `--grow --raid-devices N+1`; the reshape runs in the background (raid0/raid10 and non-mdadm technologies refused) |
| GET | `/storage/available-home-disks` | auth | `handle_available_home_disks` (valid session, not admin) |
| GET | `/storage/disks/{name}/smart/capabilities` | auth | `handle_disk_smart_capabilities` — whether `smartctl` supports this disk |
| GET | `/storage/disks/{name}/smart/test` | auth | `handle_disk_smart_test_status` — running self-test status |
| POST | `/storage/disks/{name}/smart/test` | admin | `handle_disk_smart_test_start` — start a SMART self-test (`test_type` is whitelisted to `short`/`long`/`conveyance`); logs the admin actor to the event log |

> The former `/storage/pools*` endpoints (`handle_pools_list`,
> `handle_advanced_pool_create`, `handle_pool_get`, …) were **removed** in the
> 2026-09-07 RAID↔Storage re-scope: Storage manages every virtual disk via
> `virtual-disks` above, RAID only creates. `src/pools.rs` (`PoolService`) is
> still reached through the virtual-disk and cache paths.

## Disk Encryption — `encryption/*`

LUKS2 encryption for a single available (unassigned) block device — not RAID
arrays or ZFS pools. Unlock is always manual: no key material is ever stored on
the server, so a volume stays locked after every reboot until an admin re-enters
the passphrase through this API. See [guides/encryption.md](./guides/encryption.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/encryption/volumes` | auth | `handle_encryption_list` (includes live locked/unlocked status) |
| GET | `/encryption/available-devices` | auth | `handle_encryption_available_devices` |
| POST | `/encryption/volumes` | admin | `handle_encryption_create` (`{ device, filesystem, label, passphrase }`) |
| POST | `/encryption/volumes/{id}/unlock` | admin | `handle_encryption_unlock` (`{ passphrase }`) |
| POST | `/encryption/volumes/{id}/lock` | admin | `handle_encryption_lock` |
| DELETE | `/encryption/volumes/{id}` | admin | `handle_encryption_delete` (`{ confirm: true }`, erases LUKS key slots) |

## Explorer — `explorer/*` (auth)

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/explorer/list` | auth | `?path=`; response has `truncated: bool` (cap `MAX_DIR_ENTRIES = 10_000`) |
| GET | `/explorer/roots` | auth | `handle_explorer_roots` — labelled sidebar roots (`{path, kind, name}`): the session user's home, each PathGuard root, then every mounted virtual disk (`kind:"virtualdisk"`) |
| GET | `/explorer/search` | auth | `?path=&query=&max_results=` |
| GET | `/explorer/disks` | auth | `handle_explorer_disks` — mounted volumes grouped `system\|internal\|raid\|external\|cloud\|network`; per entry `mount_point`/`filesystem`/`size_bytes`/`used_bytes` + optional `label`/`vendor`/`model`/`serial` |
| POST | `/explorer/mkdir` | auth | `?path=` |
| POST | `/explorer/delete` | **admin** | `?path=` — permanent; internal use only now, see `/explorer/trash` |
| POST | `/explorer/rename` | auth | `?path=&new_name=` — **both** paths are path-guard validated |
| POST | `/explorer/save` | auth | `handle_explorer_save` — inline text/code editor, atomic temp-file + rename, 5 MB cap |
| POST | `/explorer/copy` | auth | `?source=&destination=&strategy=overwrite\|keep_both\|skip`; directories return `{job_id, sse_url}`; every file is SHA-256 verified end-to-end |
| POST | `/explorer/move` | auth | `?source=&destination=&strategy=`; same job pattern; source deleted only after a verified copy |
| GET/POST | `/explorer/permissions` | auth | `?path=&mode=` — read/set POSIX mode |
| GET | `/explorer/preview` | auth | text/image preview; `previewable: false` for non-text binaries |
| POST | `/explorer/extract` | auth | `?path=&destination=` |
| POST | `/explorer/compress` | auth | JSON body `{ paths: string[], destination, format? }` |
| GET | `/explorer/download` | auth \| ticket | raw file download; `?ticket=` (single-use, from `/explorer/download-ticket`) *or* `x-session-token` header — **never** the session token in a URL; HTTP `Range` (206), `X-Content-Sha256` digest header for files ≤ 64 MB; 429 if the transfer quota is exhausted |
| POST | `/explorer/download-ticket` | auth | `?path=` — mint a single-use 60 s download ticket (checks path + download quota) |
| GET | `/explorer/thumbnail` | auth \| ticket | image thumbnail; `?ticket=` (multi-use, directory-scoped) or session header |
| POST | `/explorer/thumbnail-ticket` | auth | `?path=` — mint a multi-use 5 min thumbnail ticket for a directory |
| POST | `/explorer/upload` | auth | multipart `file` + `?path=` + `?strategy=`; `fsync`'d before success; SHA-256 in `checksums[]`, `skipped[]` for `strategy=skip`; optional `X-Expected-Sha256` header (single-file) |
| POST | `/explorer/upload/session` | auth | `?path=&filename=&size=` — open a resumable upload session |
| DELETE | `/explorer/upload/session` | auth | `?session_id=` — abort a resumable upload, remove the partial temp file |
| POST | `/explorer/upload/chunk` | auth | raw body + `?session_id=&offset=` — write one chunk at the server's authoritative offset (409 + current offset on mismatch) |
| POST | `/explorer/upload/complete` | auth | `?session_id=` — fsync + SHA-256 + atomic rename into place, then debit the quota |
| POST/GET/DELETE | `/explorer/trash` | auth | `?path=` (POST) — move to trash / list trash / empty trash |
| POST | `/explorer/trash/restore` | auth | `?id=` (cross-device EXDEV fallback) |
| DELETE | `/explorer/trash/{id}` | auth | permanently delete one trashed item |
| POST | `/explorer/format` | admin | `{ device, fstype, label? }` — format a block device as `ext4` or `exfat` (`mkfs.*` under `sudo`); `fstype` allow-list and the device validated against the available-disk list **before** any `mkfs` runs |
| GET | `/explorer/history` | auth | `?path=` — **Rewind**: the ZFS snapshot timeline covering that file/folder, oldest→newest, each with a read-only `snapshot_path` under `.zfs/snapshot/`; empty outside ZFS |
| POST | `/explorer/watch/ticket` | auth | `?path=` — mint a single-use ticket for the filesystem-watcher SSE connection (EventSource cannot set custom headers) |
| GET | `/explorer/watch` | auth \| ticket | `?ticket=&path=` — SSE stream (`change` events filtered to the watched directory + `heartbeat` every 30 s) |
| GET | `/explorer/snapshots` | auth | `?path=` — lists ZFS snapshots covering that path with a `browse_path` under `.zfs/snapshot/…` (read-only browsing) |
| POST | `/explorer/eject` | admin | `?path=<mount>` — safe-eject a removable drive (guarded to removable block devices only) |
| GET | `/explorer/shares` | auth | list the caller's "share by link" links |
| POST | `/explorer/shares` | auth | `{ path, mode?, expires_days? }` — create a download link to a file (`mode`: `anyone` \| `signed_in`; default 30-day expiry) |
| DELETE | `/explorer/shares/{token}` | auth | revoke a link (owner or admin) |
| GET | `/share/{token}` | public | **public** download (outside `/api`, before the SPA fallback); `signed_in` links need the `x-session-token` header; HTTP `Range` (206) + `X-Content-Sha256` for files ≤ 64 MB; per-token bandwidth bucket + global concurrency cap |

Paths are guarded by `config::PathGuard` (`AppState.path_guard`). See
[File Explorer](./guides/explorer.md) for the trash/compress/progress-job
design and everything found+fixed during live verification.

### Transfer integrity

Copy, move (cross-filesystem fallback) and upload guarantee the stored bytes
match the source:

- **Copy/move** (`FileBrowser::copy_file_verified` in `src/browser.rs`) streams
  the source while computing its SHA-256, `fsync`s the destination, then reads
  the destination back and compares hashes. On any mismatch the corrupt
  destination is deleted and `BrowserError::IntegrityMismatch` is returned — so
  a move never deletes the source unless a byte-identical copy is confirmed on
  disk. Symlinks are recreated as symlinks (never dereferenced), and permission
  bits, ownership (uid/gid) and mtime are preserved (best-effort, like
  `rsync -a`).
- **Upload** (`handle_explorer_upload`) `flush`es + `fsync`s each file before
  reporting success, returns each file's SHA-256 in `checksums[]`, and enforces
  an optional client `X-Expected-Sha256` header.
- **Cache mover** (`src/cache.rs`) uses `rsync -a`, which verifies every block
  and only removes source files it successfully transferred; rsync exit code 24
  ("vanished source files") is treated as success rather than failing the run.

## Shares — `shares/*` (all `admin`)

Every Shares endpoint (list, get, config, service control, and ACL
permissions) requires an **admin** session. A past audit found `handle_shares_list`
and `handle_share_get` accepted any non-empty `X-Session-Token`; they now use
`require_admin` (`src/handlers/shares.rs`).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/shares` | admin | `handle_shares_list` |
| POST | `/shares` | admin | `handle_share_create` |
| GET | `/shares/{name}/{share_type}` | admin | `handle_share_get` |
| PUT | `/shares/{name}/{share_type}` | admin | `handle_share_update` |
| PUT | `/shares/{name}/{share_type}/toggle/{enabled}` | admin | `handle_share_toggle` |
| DELETE | `/shares/{name}/{share_type}` | admin | `handle_share_delete` |
| GET/PUT | `/shares/smb\|nfs\|ftp\|webdav\|sftp/config` | admin | service config |
| GET | `/shares/sftp/users` | admin | users with SFTP access + key status (separate `homevault-sftp.service`, see [Shares guide](./guides/shares.md)) |
| PUT | `/shares/sftp/users/key` | admin | set a user's SFTP public key `{ username, public_key }` |
| POST | `/shares/sftp/rebuild` | admin | reconcile SFTP jails against current ACL grants |
| POST | `/shares/{share_type}/start\|stop\|restart` | admin | service control |
| GET/POST/DELETE | `/shares/{name}/{share_type}/permissions` | admin | per-user/group ACL |
| POST | `/shares/{name}/{share_type}/permissions/sync` | admin | re-apply ACLs |
| GET | `/users/{username}/permissions` | admin | `handle_user_permissions` |
| GET | `/shares/access` | admin | `handle_list_access` — every per-member resource grant (optional `?member=`) |
| GET | `/shares/access/mine` | auth | `handle_my_access` — the caller's own effective access (`all: true` for admins) |
| POST | `/shares/access` | admin | `handle_grant_access` — `{member, kind, resource, permission?}` |
| DELETE | `/shares/access` | admin | `handle_revoke_access` — `?member=&kind=&resource=` |
| GET | `/shares/access/groups` | admin | `handle_list_group_access` — every group-level grant (optional `?group=`) |
| POST | `/shares/access/groups` | admin | `handle_grant_group_access` — `{group, kind, resource, permission?}` |
| DELETE | `/shares/access/groups` | admin | `handle_revoke_group_access` — `?group=&kind=&resource=` |

`/shares/access` is the **fine-grained per-member** layer (apps / folders /
shares), separate from the POSIX-ACL share permissions above. `kind` ∈ {`app`,
`folder`, `share`}, `permission` ∈ {`read`, `write`, `full`}. See
[Users](./guides/users.md) for the server-side filter contract.

`share_type` ∈ {`smb`, `nfs`, `ftp`, `webdav`, `sftp`}. Permission grant applies the
real POSIX ACL **before** the DB write; revoke deletes the DB row even if the ACL
removal fails (see `src/permissions.rs`).

**Validation (2026-10-21).** Share values are embedded verbatim in root-owned
config files, so they are validated before any privileged write and a rejection
is a `400` naming the field:

- `name`: ASCII letters/digits/`-`/`_`/`.` (≤ 64), no leading `.`, no `..`, not
  a Samba-reserved section (`global`, `homes`, `printers`, `admin$`).
- `comment`: single line (no control characters), ≤ 200 chars.
- `path`: absolute, no whitespace/control/`( ) , # ;`, no `..`.
- `options` (NFS): whitespace-free tokens, no `( ) , # " '`, ≤ 16 entries —
  they are written as `path *(opt1,opt2)`.
- SMB global config: one-line `workgroup` (≤ 15), `security_mode` ∈
  {`user`,`share`,`domain`,`server`,`ads`}, octal `create_mask`/`directory_mask`.
- FTP config: single-line `welcome_message`, non-zero ports, `passive_ports` =
  `min` or `min-max` with min < max, `max_clients` ≥ 1.

`PUT /shares/{name}/nfs/toggle/{enabled}` rewrites the export line
(`# homevault-nfs-disabled:<name>` tag) and reloads `exportfs -ra`; the state
survives a reboot. A share name matching several exports in `/etc/exports`
answers `400` listing the colliding paths instead of silently editing all of
them. All share operations run on the blocking pool under a timeout
(`spawn_blocking` + 30 s, SFTP jail rebuild 120 s) — a hung
`systemctl`/`mount` returns `{ success:false, message }`, never an indefinitely
pending request.

## Network — `/api/network/*` (router in `src/handlers/network.rs`)

Read endpoints `auth`; config/state/WiFi mutations `admin`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/network/interfaces` | auth |
| GET | `/network/interfaces/{name}` | auth |
| GET | `/network/interfaces/{name}/config` | auth |
| PUT | `/network/interfaces/{name}/config` | admin — applies live, persists (netplan/ifupdown), then `netplan apply` + wire verify; a failed reconcile rolls everything back and answers `{ success:false, message }` |
| PUT | `/network/interfaces/{name}/state` | admin |
| GET | `/network/wifi/scan` | auth |
| GET | `/network/wifi/status` | auth |
| POST | `/network/wifi/connect` | admin |
| POST | `/network/wifi/disconnect` | admin |
| GET | `/network/diagnostics` | auth |
| GET | `/network/summary` | auth |
| GET | `/network/public-ip` | auth |
| GET | `/network/services` | auth | active network services |

## mDNS / Zeroconf — `mdns/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/mdns/status` | auth | `{ available, running, host_name, fqdn, http_url, services[] }` — the `homevault.local` address + advertised share types. Publishing itself is automatic (`src/mdns.rs` reconciler); see [guides/mdns.md](./guides/mdns.md). |

## Chat (AI Assistant) — `chat/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| POST | `/chat/send` | auth | chat with local Ollama model |
| POST | `/chat/send/stream` | auth | streaming chat — SSE (RAG-enriched prompt, see [Chat guide](./guides/chat.md)) |
| GET | `/chat/models` | auth | list installed models |
| POST | `/chat/models/pull` | admin | pull a model |
| GET | `/chat/models/recommend` | auth | recommendations — the 2026-07-19 docs audit found this handler took no session check; it now takes `HeaderMap` like the rest of the file and answers 200 + `{"success":false}` when unauthenticated (the `/chat/send` convention). |
| GET | `/chat/model` | auth | current model |
| PUT | `/chat/model` | admin | set current model |

The system prompt is built per the request `language` (defaults to English); replies
respect the user's UI language. When the RAG knowledge base is indexed, the system
prompt is automatically enriched with relevant context chunks.

## RAG Knowledge Base — `rag/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/rag/status` | auth | index status: last indexed timestamp, doc/chunk counts, embedding model |
| POST | `/rag/index` | admin | triggers full background reindex (catalog + features + i18n) |
| POST | `/rag/search` | admin | semantic search test — `{ query, top_k }` → results + context string |
| GET | `/rag/documents` | admin | list all indexed documents (id, source, title, language) |

Embedding model is `nomic-embed-text` (hardcoded). Admin must pull it before
indexing. Embeddings stored as f32 BLOBs; similarity search is cosine in Rust.

## External Agents (MCP) — `mcp/*` + `POST /mcp`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/mcp/status` | admin | kill-switch state, global permissions, token prefixes (never secrets) |
| PUT | `/mcp/status` | admin | `{enabled}` — disabling keeps tokens/permissions |
| PUT | `/mcp/permissions` | admin | replaces the global permission set (everything defaults to denied) |
| POST | `/mcp/tokens` | admin | mints `hv_mcp_…` (cap 20); full value returned exactly once |
| DELETE | `/mcp/tokens/{id}` | admin | revoke |
| POST | `/mcp` (no `/api` prefix) | Bearer `hv_mcp_…` | MCP Streamable HTTP: `initialize`, `tools/list`, `tools/call`, `ping` |

Disabled/invalid/revoked tokens all answer the same 401. The tool set is
filtered per request from the permission snapshot (system info, apps,
catalog, files, VMs, knowledge search, reboot). See
[External Agents](./guides/mcp.md).

## Restic (Backup) — `restic/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/restic/available` | auth | is `restic` installed |
| GET | `/restic/version` | auth | |
| GET | `/restic/repos` | auth | `?include_password=` (admin only when true) |
| POST | `/restic/repos` | admin | create repo |
| GET/PUT/DELETE | `/restic/repos/{id}` | auth/admin | get/update/delete |
| GET | `/restic/repos/{id}/snapshots` | auth | |
| GET | `/restic/repos/{id}/stats` | auth | |
| GET | `/restic/repos/{id}/jobs` | auth | backup job history |
| POST | `/restic/repos/{id}/unlock` | admin | |
| POST | `/restic/repos/{id}/prune` | admin | |
| POST | `/restic/backup` | admin | run backup (SSE progress) |
| GET | `/restic/reminders` | auth | backup reminders — per repo: `last_backup_at` + `days_since` + `stale`, so the UI can nag when a repository hasn't been backed up recently |
| GET | `/restic/restore-plan` | auth | `?repo_id=&snapshot=` — pre-flight space check `{ size_bytes, available_bytes, fits }` (snapshot restore size vs free space on the destination); the restore dialog debounces this before committing |
| POST | `/restic/restore` | admin | |
| POST | `/restic/check` | auth | |
| POST | `/restic/forget` | admin | apply retention |
| POST | `/restic/snapshots/delete` | admin | |
| GET | `/restic/schedules` | auth | |
| POST | `/restic/schedules` | admin | |
| PUT/DELETE | `/restic/schedules/{id}` | admin | |
| PUT | `/restic/schedules/{id}/enabled` | admin | |
| GET | `/restic/retention-policies` | auth | |
| POST | `/restic/retention-policies` | admin | |
| PUT/DELETE | `/restic/retention-policies/{id}` | admin | |

Backup jobs are marked `completed`/`failed` in the DB on both success and failure
paths (`handle_restic_backup_create`).

## Docker — `docker/*` (almost entirely `admin`)

Containers, images, networks, compose projects. `handle_docker_available` is
the one deliberate `auth`-only exception (it just gates whether the Docker
app is shown to any logged-in user); `handle_catalog_list` (`GET
/docker/catalog`) is the other. Every other handler shown below as `auth` in
older docs actually calls `require_admin`, including the read/list ones
(`info`, `containers`, `containers/running`, `images`, `hub/search`,
container logs) — Docker exposes real host details (images, container
config, logs), so read access is admin-gated too, not merely session-gated.
Representative endpoints:

| Method | Path | Auth |
|--------|------|------|
| GET | `/docker/info` | admin |
| GET | `/docker/available` | auth |
| GET | `/docker/containers` | admin |
| GET | `/docker/containers/running` | admin |
| GET | `/docker/images` | admin |
| PUT | `/docker/containers/{name}/start\|stop\|restart\|pause\|unpause` | admin |
| PUT | `/docker/containers/{name}/rename` | admin | body `{name}`; `ComposeManager`-style name validation |
| POST | `/docker/containers/{name}/kill` | admin | body `{signal?}` — whitelisted (`ALLOWED_SIGNALS`), unknown ⇒ SIGKILL |
| POST | `/docker/containers/{name}/update` | admin | body `{memory_limit?, cpu_limit?, restart_policy?}` — in place, no recreate |
| POST | `/docker/containers/{name}/recreate` | admin | pull + rebuild from `inspect`, rename-rollback on failure |
| POST | `/docker/containers/{name}/exec` | admin | body `{command: string[], timeout_secs?}` → `{stdout, stderr, exit_code}` |
| GET | `/docker/containers/{name}/inspect` | admin | `ContainerDetail`: state, limits, mounts, ports, networks, env, labels, compose ids |
| GET | `/docker/containers/{name}/logs` | admin | merged stdout+stderr, `?tail=` (1–5000), `?timestamps=` |
| POST | `/docker/containers/{name}/logs/ticket` | admin | single-use 30 s ticket for the SSE stream (EventSource can't send the token header) |
| GET | `/docker/containers/{name}/logs/stream` | ticket | SSE `log` / `end` events; 401 on missing, expired or mismatched ticket |
| POST | `/docker/containers` | admin |
| DELETE | `/docker/containers/{name}` | admin |
| POST | `/docker/images/pull` | admin |
| POST | `/docker/images/build` | admin | body `{context, dockerfile?, tag, no_cache?, pull?, build_args?}`; context through `PathGuard` |
| DELETE | `/docker/images/{image_id}` | admin |
| GET | `/docker/hub/search` | admin |
| GET/POST | `/docker/volumes` | admin | list (driver, mountpoint, `in_use`, `used_by`) / create `{name, driver?}` |
| DELETE | `/docker/volumes/{name}` | admin | `?force=` for a volume still attached to a container |
| GET | `/docker/system/df` | admin | `docker system df` rows + totals |
| POST | `/docker/prune` | admin | body `{target, all?}`; target ∈ `images\|volumes\|containers\|networks\|builders\|system`, anything else rejected **before** the daemon is called |
| GET | `/docker/registries` | admin | signed-in hosts only (`auths` keys of `~/.docker/config.json`) |
| POST | `/docker/registries/login\|logout` | admin | login uses `--password-stdin` |
| GET/PUT | `/docker/daemon/config` | admin | read / validated write of `daemon.json` (+ `systemctl restart docker`) |
| GET/POST/DELETE | `/docker/networks` (+ `/{name}`, `/prune`, `/connect`, `/disconnect`) | admin |
| GET/POST/PUT/DELETE | `/docker/compose/projects` (+ `/{name}/config\|start\|stop\|restart\|update\|services`) | admin | `update` = `compose pull` + `up -d` |
| GET | `/docker/catalog` | auth | manifest-driven app list (15 built-in packages + community stores) + `installed`/`installed_port` |
| GET | `/docker/catalog/{id}/icon` · `/gallery/{n}` | auth | package icon / screenshot (`id` is namespaced `<store>:<app>`, URL-encoded) |
| POST | `/docker/catalog/{id}/install` | admin | renders the package's compose template into a project, starts it, writes `.install.toml`, starts the per-app proxy. Body `{port?, alternatives?, environment?}`; hard-gates compatibility, settings and dependencies |
| GET | `/docker/catalog/{id}/install-review` | auth | pre-install review: dependency plan (satisfiers, providers, selection), declared environment, compatibility verdict |
| GET | `/docker/catalog/{id}/logs?tail=N` | auth | tails the installed app's compose logs (1–2000, default 200) |
| DELETE | `/docker/catalog/{id}` | admin | stops + removes the project, stops the proxy |
| POST | `/docker/catalog/{id}/proxy` | admin | toggle the per-app session-cookie auth gate |
| GET | `/docker/catalog/updates` · POST `/docker/catalog/{id}/update` | admin | registry-digest update detection (read-only) / `compose pull` + re-render + `up -d` |
| POST | `/docker/catalog/update-all` | admin | sequential update queue over every pending update → `{updated[], failed[]}` |
| GET/POST/DELETE | `/docker/stores` (+ `/{id}/sync`) | admin | community catalog git repos |
| POST | `/docker/daemon/start` | admin | start the Docker daemon via systemd (**also enables it for boot persistence**) |
| GET | `/docker/daemon/logs` | admin | last 100 lines of the daemon's journal, for diagnosing why it is down |
| GET | `/docker/port-check` | admin | `?port=N` — is a host port free to publish a container on (host `ss` + other containers + HomeVault's web port); re-checked server-side on create/install |

**App catalog (2026-09-03 overhaul):** `src/docker_catalog.rs::CATALOG` is gone.
Apps are packages — a `homevault-app.toml` manifest + a `docker-compose.yml`
template — from the embedded `catalog/` dir plus admin-added community stores.
Wire ids are namespaced `<store>:<app>`; the install key (compose project + data
dir) is the **bare** app id for the built-in `homevault` store, `<store>-<app>`
otherwise. Each installed app gets a dedicated reverse-proxy listener
(47000–47999, `src/app_proxy`) with an optional app-scoped-cookie auth gate;
the app's own published port stays bound to `0.0.0.0` on purpose (the proxy is
an extra entry point, not a lockdown). The proxy's auth gate accepts only the
proxy-scoped `hv_app_session` minted by `GET /app-auth` — never the API
session. See [Docker](./guides/docker.md) and
[Apps Center](./guides/apps-center.md).

Every Compose project-name path parameter is validated by
`ComposeManager::validate_name` before it's joined onto a filesystem path
(path-traversal fix, 2026-07-16 audit).

## Cloud Sync — `rclone/*`

> Previously **every** handler took only `State(_state)` — no session check
> at all. `GET /rclone/config` returns the raw content of the rclone config
> file the server resolved for the request — `rclone_config_path()`: an explicit
> `RCLONE_CONFIG`, else the first existing of rclone's locations, else
> `/etc/rclone.conf`;
> rclone's "obscured" secret encoding is trivially reversible, so this was an
> unauthenticated plaintext credential leak for every configured remote (S3
> keys, OAuth tokens, FTP/SFTP passwords). `sync`/`copy` support a `"local"`
> path type with no restriction, so combined with the missing auth this was
> also an unauthenticated arbitrary-file-read-and-write primitive. Fixed:
> reads use `require_auth`, mutations (including anything that can start a
> transfer, mount a filesystem, or read/write the config) use `require_admin`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/rclone/status` | auth |
| GET | `/rclone/remotes` | auth |
| GET | `/rclone/providers` | auth |
| GET | `/rclone/mounts` | auth |
| POST | `/rclone/mount/start` | admin |
| POST | `/rclone/mount/stop` | admin |
| POST | `/rclone/sync` | admin |
| POST | `/rclone/copy` | admin |
| GET | `/rclone/config` | admin — returns raw config, same sensitivity bar as Restic's `?include_password=true` |
| POST | `/rclone/config` | admin |
| POST | `/rclone/remotes` (add) | admin |
| DELETE | `/rclone/remotes/{name}` | admin |
| GET | `/rclone/jobs` (list) | auth — persisted sync jobs |
| POST | `/rclone/jobs` (create) | admin |
| DELETE | `/rclone/jobs/{id}` | admin |
| POST | `/rclone/jobs/{id}/run` | admin — one-shot "Run now" reusing the `execute_sync` core (no scheduler; the job never auto-runs) |

`POST /rclone/crypt` was removed — it was a fake stub (always returned a
canned success string without doing anything) with zero frontend callers.

See [Cloud Sync](./guides/cloud-sync.md) for the mount-deserialization fix,
the config-injection fix, and what was verified live.

## Syncthing — `syncthing/*` (all `admin`)

Each handler calls a local `require_admin` — this closes a previously-real gap
where none of these handlers validated a session at all; see
[Syncthing](./guides/syncthing.md) for what was found and fixed.

| Method | Path | Notes |
|--------|------|-------|
| GET | `/syncthing/status` | instance status |
| GET | `/syncthing/config` | fetched config |
| GET | `/syncthing/devices` | devices |
| GET | `/syncthing/folders` | folders |
| GET | `/syncthing/connections` | live connections |
| GET | `/syncthing/errors` | sync errors |
| POST | `/syncthing/restart` | restart the local instance |
| POST | `/syncthing/pause` | pause one device |
| POST | `/syncthing/resume` | resume one device |
| POST | `/syncthing/scan` | rescan one folder |

## Downloader — `downloader/*` (`aria2`, all `admin`)

Every Downloader endpoint calls `require_admin_request`, so **all** are
admin-only. There are no read-only `auth` endpoints here.

| Method | Path | Notes |
|--------|------|-------|
| GET | `/downloader/status` | daemon status + globals |
| GET | `/downloader/global-stat` | aria2 global stats |
| GET | `/downloader/tasks` | active/waiting/done tasks |
| GET | `/downloader/task/{gid}` | one task |
| POST | `/downloader/add` | enqueue a download |
| POST | `/downloader/start` | start the daemon |
| POST | `/downloader/stop` | stop the daemon |
| POST | `/downloader/pause` | pause a task |
| POST | `/downloader/unpause` | unpause a task |
| POST | `/downloader/remove` | remove a task |

## Snapshots

> The standalone `/snapshots/{pool}` endpoints and `src/handlers/snapshots.rs`
> were **removed** — every handler there took no `State`/`HeaderMap`, so all
> six endpoints (including destructive delete/rollback) were reachable with
> **no session token whatsoever**. Snapshot functionality now lives where it
> is scoped:
>
> | Area | Endpoints |
> |------|-----------|
> | Explorer (ZFS, read-only browsing) | `GET /explorer/snapshots?path=` (lists covering snapshots with a `browse_path` under `.zfs/snapshot/…`) |
> | Backups (restic) | `GET /restic/repos/{id}/snapshots`, `POST /restic/snapshots/delete` |
> | Virtual machines | `GET/POST /vm/{name}/snapshots`, `POST /vm/{name}/snapshots/{snap}/restore`, `DELETE /vm/{name}/snapshots/{snap}` |
>
> See [Storage](./guides/storage.md) for the Btrfs/ZFS rollback limitation
> (Btrfs rollback of the default mounted subvolume isn't supported and returns
> a clear error, not a silent no-op).

## Progress (SSE) — `progress/*` (`auth`)

`GET /progress/{job_id}` (SSE stream), `GET /progress/jobs`, `GET
/progress/{job_id}/status`, `DELETE /progress/{job_id}` (cancel). Long-running jobs
(backup, restore, clone, etc.) emit `progress` / `done` / `error` / `heartbeat`
events; the SPA subscribes via `EventSource` (`api.subscribeToProgress`).
`/status` answers the job's **real last event** (latest `%`, message, byte
counters and state — `completed`/`failed` included) and a 400 for an id that was
never created or was already removed.

Cancelling runs the job producer's registered hook
(`ProgressManager::register_cancel_hook`) — that is what actually stops the
work; the job then reports `state: "cancelled"`. The response is
`{job_id, cancelled, message}`, and an unknown id is a `400` rather than the
old unconditional "Job cancelled" (which left a `sudo ddrescue` clone running
after the UI said it was stopped). In a `ProgressEvent`, `job_type` is a
tagged object: `{"type":"backup"}` for the built-in kinds,
`{"type":"custom","name":"explorer_copy"}` for a feature job (the enum uses
serde **adjacent** tagging — plain internal tagging cannot serialize the
`Custom` newtype variant, which used to empty the SSE payload for every feature
job, see [guides/explorer.md](./guides/explorer.md)).

## Activity feed — `GET /api/activity` (`auth`)

Unified list for the desktop activity taskbar. Merges the active
`PROGRESS_MANAGER` jobs (backup/restore/verify, cache mover, antivirus scan,
Explorer copy/move, Docker install/update, ddrescue/photorec, RAG reindex,
Ollama model pull, cloud sync) with aria2's active downloads (admin only, and
only when aria2 is already running — the call never starts it). Non-admins see
only jobs they own. `ApiResponse<ActivityTask[]>` where `ActivityTask =
{ id, kind, label, progress: number|null, detail: string|null }`; `kind` is a
stable slug localised client-side. See [guides/activity.md](./guides/activity.md).

## Rescue — `/rescue/*` (`admin`)

`tools`, `devices`, `ddrescue/mapfile` (`GET`); `ddrescue/clone`,
`ddrescue/status/{log}`, `testdisk/analyze`, `testdisk/rebuild`,
`photorec/recover`, `mdadm/diagnose`, `mdadm/repair` (`POST`). Implemented in
`src/handlers/rescue.rs` + `src/rescue/`. Full semantics in
[guides/rescue.md](./guides/rescue.md).

- `ddrescue/status/{log}` takes a **mapfile path** and answers
  `{running, progress, bytes_copied, bytes_total, speed, message}`. The path is
  confined to `/tmp`, `/var/tmp` and `paths::rescue_dir()` and must be an
  existing regular file that is not a symlink (≤ 16 MiB) — a missing file is
  not an error, it means no mapfile has been written yet. `ddrescue/mapfile`
  reports the directory the per-job default mapfiles live in.
- Only options the packaged tools can honour are accepted; anything else is a
  `400` naming the field (see the option table in `guides/rescue.md`). That
  includes TestDisk's `deep_search`/`log_path`/`force` and PhotoRec's
  `keep_broken`/`max_size`, which used to be accepted and ignored.
- A clone is refused when the destination is the same device, is in use
  (mounted / active swap / md / LVM / dm-crypt member) or is smaller than the
  source.
- Clones and carves can be stopped: `DELETE /progress/{job_id}` now runs the
  job's cancellation hook, which SIGTERMs the tool's process group.

## VPN — `/vpn/*` (all `admin`)

Every VPN endpoint requires an **admin** session — the whole surface is
privileged (it writes `/etc`, brings up `wg0` and launches root daemons), so
there are no read-only `auth` endpoints here. Router in `src/handlers/vpn.rs`,
engine in `src/vpn.rs`; full semantics in [guides/vpn.md](./guides/vpn.md).

| Method | Path | Notes |
|--------|------|-------|
| GET | `/capabilities` | `{capabilities:{wireguard,openvpn}}` — which backends are installed |
| GET | `/connections` | legacy flat list — `{connections: [...]}` |
| GET | `/connections/{id}` | `{connection}` |
| GET | `/wireguard/server` | server status; **provisions `wg0` on first call** (`10.8.0.1/24`, port 51820) |
| GET | `/wireguard/peers` | provisioned devices + live handshake status (`connected` = handshake < 180 s) |
| POST | `/wireguard/peers` | add device — `{name}` only; response carries the one `client_config` |
| GET | `/wireguard/peers/{id}/config` | re-display the device's client config/QR (keys stored per migration 026) |
| DELETE | `/wireguard/peers/{id}` | revoke device (live + persisted) |
| POST | `/openvpn` | upload profile — `{name,config_content}`; **content-validated** (no exec directives / `script-security > 1`, else 400 naming the directive), **409** if the same name's profile is currently running |
| POST | `/openvpn/{name}/start` | start + **verify the tunnel connects within a 10 s grace** (log marker), else the daemon is killed and the API answers `success:false` with the log's error line; body `{username,password}` optional (`--auth-user-pass`) |
| POST | `/openvpn/{name}/stop` | kill daemon, delete its `.log` and `.auth` files |
| DELETE | `/openvpn/{name}` | stop, then delete profile |

Payloads use the app's standard `{success, message?, data}` envelope. The
OpenVPN start request intentionally blocks up to ~10 s (verify-then-rollbac
k); the frontend shows a per-card connecting spinner meanwhile.

## Virtual Machines — `/vm/*` (all `admin`)

Incus-backed VMs and system containers. Every endpoint requires an **admin**
session (Incus uses a root socket). Router for the console WebSocket in
`src/handlers/vm.rs::create_vm_router`; the REST routes are registered directly
in `src/web/mod.rs`. Full detail in [Virtual Machines](./guides/vm.md).

| Method | Path | Notes |
|--------|------|-------|
| GET | `/vm/available` | `{ available }` — Incus daemon reachable |
| GET | `/vm/os-options` | curated OS catalogue (`OsOption[]`); no Incus needed |
| GET | `/vm/list` | `VmInfo[]` — status, IPv4, snapshot count (one `recursion=2` call) |
| POST | `/vm/create` | `CreateVmRequest` → `{ name, status }`; image pull runs as a `vm_create` progress job |
| GET | `/vm/{name}` | `VmDetail` |
| DELETE | `/vm/{name}` | stops first if running |
| PUT | `/vm/{name}/start` \| `/stop` \| `/restart` | waits for the operation |
| PUT | `/vm/{name}/config` | `{ cpu_cores?, memory_mb? }` |
| GET/POST | `/vm/{name}/snapshots` | list / create `{ name, stateful? }` |
| POST | `/vm/{name}/snapshots/{snap}/restore` | stop → restore → restart if it was running |
| DELETE | `/vm/{name}/snapshots/{snap}` | |
| POST | `/vm/console/ticket` | `{ name, cols?, rows? }` → `{ ticket }` (single-use, 20 s) |
| GET | `/vm/console/ws` | `?ticket=…` WebSocket → interactive shell (xterm) |
| POST | `/vm/{name}/screen/ticket` | `{ ticket }` — ISO-install VM's graphical screen (must be running + have a screen) |
| GET | `/vm/screen/ws` | `?ticket=…` WebSocket → VM VNC (noVNC), subprotocol `binary` |
| GET | `/vm/isos` | managed ISO store (`{DATA_DIRECTORY}/vm/isos`) — `IsoEntry[]` |
| POST | `/vm/isos/download` | `{ url, name? }` — fetch an https `.iso` (progress job) |
| POST | `/vm/isos/virtio` | one-click latest `virtio-win.iso` |
| DELETE | `/vm/isos/{name}` | remove a managed ISO |

`CreateVmRequest`: `name` (1–63 `[a-z0-9-]`, lower-cased), `size`
(`small`|`medium`|`large`), `cpu_cores?`/`memory_mb?`/`disk_gb?`, plus **one of**:
ready-made — `kind` (`vm`|`container`), `os` (catalogue id), `username`
(default `admin`), `password` (6–128, seeded via cloud-init as a SHA-512 hash);
or ISO — `iso_path` (absolute `.iso`, exists, ≥ 10 MB), `iso_os`
(`linux`|`windows`), `drivers_iso_path?`, `secure_boot?`.

## Wallpapers

**No API.** The upload feature was removed 2026-09-04. The 44 bundled wallpapers
are static assets served at `GET /wallpapers/{*path}` (registered before the SPA
fallback). See [Wallpapers](./guides/wallpapers.md).

## Avatars

`GET /avatars/preview` (public, raw SVG), `POST /auth/me/avatar` (auth, set seed).
User avatars served at `GET /users/{username}/avatar.svg`.

## Terminal — `terminal/*` (all `admin`)

Router in `src/handlers/terminal_pty.rs`, nested at `/api/terminal`. The PTY
WebSocket requires a single-use ticket (long-lived tokens never appear in URLs).

| Method | Endpoint | Notes |
|--------|----------|-------|
| POST | `/terminal/ticket?tab=` | issue a single-use WS ticket |
| POST | `/terminal/close?tab=` | tear down one shell tab |
| GET | `/terminal/cwd?tab=` | real shell cwd (via `/proc/<pid>/cwd`) — feeds completion + prompt |
| GET | `/terminal/ls?tab=&path=` | directory listing for the completion popup |
| WS | `/terminal/ws?ticket=` | the shell (ticket redeemed once) |

## Disk quotas — no endpoints

The per-group filesystem-quota feature (`quotas/*` routes, `handlers/quotas.rs`)
was **removed 2026-09-09**. The only disk quota left is the **per-user
home-directory size limit** (`setquota -u`), which has no dedicated endpoint —
it rides on the Users home-folder calls (`POST /users`, `PUT /users/{u}/full`,
`PUT /users/{u}/home`). Distinct from **transfer quotas** (per-user/group
upload/download caps, `transfer-quotas/*`). See
[Disk Quotas](./guides/quotas.md).

## Notifications — `notifications/*` (`auth`)

Broadcast-style system notifications (health alerts, backup results), populated
by the background engine in `src/notification_engine.rs`. See
[guides/notifications.md](./guides/notifications.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/notifications` | auth | `handle_notifications_list` (50 most recent) |
| GET | `/notifications/unread-count` | auth | `handle_notifications_unread_count` |
| POST | `/notifications/read-all` | auth | `handle_notifications_mark_all_read` |
| POST | `/notifications/{id}/read` | auth | `handle_notification_mark_read` |
| DELETE | `/notifications/{id}` | auth | `handle_notification_delete` |
| GET | `/notifications/events/config` | admin | `handle_notification_events_config_get` — event catalog + per-event state + `webhook_format` |
| PUT | `/notifications/events/config` | admin | `handle_notification_events_config_update` — `{ events?: {id: bool}, webhook_format?: "generic"\|"slack"\|"discord" }` |

## What's New — `whats-new/*` (`auth`)

Per-user release highlights shown once per version. See
[guides/notifications.md](./guides/notifications.md) → "What's New".

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/whats-new` | auth | `handle_whats_new` — `{ version, title, highlights, seen }` |
| POST | `/whats-new/seen` | auth | `handle_whats_new_seen` — per-user acknowledgement (idempotent) |

## Client/server contract — `/api/meta` (public)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/meta` | public | `handle_meta` — `{ contract_version, server_version }` |

The SPA sends `X-HomeVault-Contract: <n>` on every API request; a present but
different value is rejected with **426 Upgrade Required** by
`middleware::contract_middleware` (`/api/meta` exempt). A missing header is
allowed for legacy clients. See [ARCHITECTURE.md](./ARCHITECTURE.md) →
"Versioned client/server contract".

## Antivirus (ClamAV) — `antivirus/*`

Read endpoints `auth`; scan/settings mutations `admin`. See
[Antivirus](./guides/antivirus.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/antivirus/status` | auth |
| GET/PUT | `/antivirus/settings` | auth / admin |
| POST | `/antivirus/scan` | admin |
| POST | `/antivirus/scan/cancel` | admin |
| GET | `/antivirus/scans` | auth |
| GET | `/antivirus/threats` | auth |
| POST | `/antivirus/threats/{id}/resolve` | admin |

## Firewall (ufw) — `firewall/*`

Read endpoints `auth`; enable/disable/rule mutations `admin`. Enabling requires
`{"confirmed": true}` in the body; mandatory SSH/app-port rules cannot be
deleted through `/firewall/rules`. See [Firewall](./guides/firewall.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/firewall/status` | auth |
| GET | `/firewall/enable/preview` | auth |
| POST | `/firewall/enable` | admin |
| POST | `/firewall/disable` | admin |
| POST/DELETE | `/firewall/rules` | admin |

## Anti-Ransomware — `ransomware/*`

Read endpoints `auth`; settings/protected-path mutations `admin`. See
[Anti-Ransomware](./guides/ransomware.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/ransomware/status` | auth |
| GET/PUT | `/ransomware/settings` | auth / admin |
| GET/POST | `/ransomware/protected-paths` | auth / admin |
| DELETE | `/ransomware/protected-paths/{id}` | admin |
| GET | `/ransomware/events` | auth |

## Cache tier — `cache/*`

Read endpoints `auth`; settings/share/mover mutations `admin`. See
[Cache](./guides/cache.md).

| Method | Path | Auth |
|--------|------|------|
| GET/PUT | `/cache/settings` | auth / admin |
| GET | `/cache/pools` | auth |
| GET/POST | `/cache/shares` | auth / admin |
| PUT | `/cache/shares/{name}` | admin |
| POST | `/cache/mover/run` | admin |
| GET | `/cache/mover/runs` | auth |

## System Journal — `journal/*`

Read endpoints `auth`; clear-logs and cleanup-settings writes `admin`. See
[System Journal](./guides/journal.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/journal/sources` | auth |
| GET | `/journal/entries` | auth |
| DELETE | `/journal/clear` | admin |
| GET/PUT | `/journal/cleanup-settings` | auth / admin |

## Transfer quotas — `transfer-quotas/*`

Per-user/per-group upload/download caps, enforced against File Explorer
transfers only (not SMB/NFS/FTP/WebDAV/SFTP shares — those are served by
separate system daemons this codebase can't meter). See
[Transfer Quotas](./guides/transfer-quotas.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/transfer-quotas/{subject_type}` | auth |
| GET | `/transfer-quotas/{subject_type}/{subject_name}` | auth |
| PUT | `/transfer-quotas/{subject_type}/{subject_name}` | admin |
| DELETE | `/transfer-quotas/{subject_type}/{subject_name}` | admin |

`subject_type` ∈ {`user`, `group`}. A user's own quota and every group they
belong to are each checked and debited independently.

## UPS / SNMP — `ups/*`, `snmp/*` (all `admin`)

Status/inventory endpoints for a locally-attached UPS (via NUT) and
SNMP-discoverable network devices. `snmp/settings` has a `PUT`; the UPS
`config/*` endpoints manage the device sections of `/etc/nut/ups.conf`
(serial / network / USB — a USB UPS is normally auto-detected by the
`usbhid-ups` `port = auto` section the installer writes). Both have real
frontend cards in Settings (`UpsCard.svelte`/`SnmpCard.svelte`) — see
[Settings](./guides/settings.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/ups/availability` | admin |
| GET | `/ups/list` | admin |
| GET | `/ups/status/{name}` | admin |
| GET | `/ups/variables/{name}` | admin |
| GET | `/ups/config` | admin — list `ups.conf` sections |
| POST | `/ups/config` | admin — add/update a section (`{name, driver, port, desc?}`; driver allow-listed, reloads NUT) |
| DELETE | `/ups/config/{name}` | admin — remove a section (reloads NUT) |
| GET | `/snmp/availability` | admin |
| GET | `/snmp/info` | admin |
| GET | `/snmp/interfaces` | admin |
| GET | `/snmp/storage` | admin |
| GET/PUT | `/snmp/settings` | admin |

## Desktop monitor state — `monitor-layout`, `weather-location` (`auth`)

Per-user persisted UI state, not tied to any specific monitor widget:
`GET/POST /monitor-layout` (drag position + visibility for every desktop
monitor panel) and `GET/POST /weather-location` (the Weather monitor's saved
city). See [Widgets](./guides/widgets.md).

## Web shortcuts — `shortcuts/*` (`auth`)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| POST | `/shortcuts/fetch-metadata` | auth | `handle_fetch_metadata` — `{url}` → `{url, title, description, icon_url, site_name}` |

The server fetches the page (SSRF-guarded: http/https only, every resolved
address must be public, no automatic redirects, 10 s / 512 KB cap). See
[Web shortcuts](./guides/shortcuts.md).

## Events (SSE) — `/api/events`, `notifications/events/ticket` (`auth`)

Same single-use-ticket pattern as the Terminal WebSocket (a long-lived token
must not travel in a URL query string): `GET /notifications/events/ticket`
(auth) issues a short-lived ticket, then the actual `GET /events?ticket=...`
SSE stream (no header-based auth — the ticket is one-time use) pushes live
notification events to the client.

## Scheduled Tasks — `scheduled-tasks/*` (admin)

The generic cron engine (commands, pool snapshots, SMART self-tests, host
power actions). See [guides/scheduled-tasks.md](./guides/scheduled-tasks.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/scheduled-tasks` | admin | `handle_tasks_list` |
| POST | `/scheduled-tasks` | admin | `handle_task_create` — payload validated at creation |
| PUT | `/scheduled-tasks/{id}` | admin | `handle_task_update` (also enable/disable) |
| DELETE | `/scheduled-tasks/{id}` | admin | `handle_task_delete` |
| POST | `/scheduled-tasks/{id}/run` | admin | `handle_task_run_now` — runs regardless of cron |
| GET | `/scheduled-tasks/{id}/runs` | admin | `handle_task_runs` — 50 newest |

## Metrics history — `metrics/*` (auth)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/metrics/history?range=1h\|24h\|7d` | auth | `handle_metrics_history` — bucketed averages (`bucket_seconds` + `points[]`, gaps as `null`s) |
| GET | `/metrics/latest` | auth | `handle_metrics_latest` — newest stored sample |

Read-only: samples are written only by the `metrics` collector loop.

## Host power — `power/*` (admin)

See [guides/power.md](./guides/power.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/power/status` | admin | `handle_power_status` — governor + capability flags |
| PUT | `/power/governor` | admin | `handle_power_governor_set` — `{ governor }` |
| POST | `/power/reboot` | admin | `handle_power_action` — host reboot |
| POST | `/power/shutdown` | admin | `handle_power_action` — host poweroff |
| POST | `/power/suspend` | admin | `handle_power_action` — host suspend |
| GET | `/power/spindown` | admin | `handle_power_spindown_get` |
| PUT | `/power/spindown` | admin | `handle_power_spindown_set` — `{ device, apm_level?, spindown_minutes? }`, applied immediately |
| POST | `/power/spindown/apply` | admin | `handle_power_spindown_apply` — re-apply all saved settings |

## Snapshot schedules — `snapshot-schedules/*` (admin)

Cron-driven pool snapshots with GFS retention. See
[guides/snapshots.md](./guides/snapshots.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/snapshot-schedules` | admin | `handle_snapshot_schedules_list` |
| POST | `/snapshot-schedules` | admin | `handle_snapshot_schedule_create` — `{ pool, cron, prefix, recursive, gfs_hourly..yearly, enabled }` |
| PUT | `/snapshot-schedules/{id}` | admin | `handle_snapshot_schedule_update` |
| DELETE | `/snapshot-schedules/{id}` | admin | `handle_snapshot_schedule_delete` — snapshots are kept |
| POST | `/snapshot-schedules/{id}/run` | admin | `handle_snapshot_schedule_run_now` — create + GFS prune now |
| GET | `/snapshot-schedules/snapshots?pool=` | admin | `handle_snapshot_pool_list` — pool snapshots, `auto` tags the schedules' own prefix |

## Health endpoint detail

`GET /health` (`handle_health_endpoint`, `src/web/mod.rs:222`) requires a
valid session, runs `HealthService::run_all_checks()` (9 checks —
authentication, storage, memory, disk_space, disk_smart, **raid_arrays**,
network, services, temperatures; `raid_arrays` reports degraded/failed mdadm
arrays as a warning/error and raises `health.raid_arrays` notifications), and
reads CPU/memory from `/proc/stat` and `/proc/meminfo`. Returns
`{ success, data: { checks[], overall_status, cpu_usage, memory_usage } }` —
note the field names are `checks`/`overall_status`, not `metrics`/`overall`.
See [Health](./guides/health.md).

## Related docs

- [Backend Structure](./BACKEND.md) — where each handler lives.
- [Frontend Structure](./FRONTEND.md) — `api.ts` client methods.
- Per-area guides under [guides/](./guides/).
