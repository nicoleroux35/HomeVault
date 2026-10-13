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
| POST | `/auth/login` | public | `handle_login` |
| POST | `/auth/logout` | auth | `handle_logout` |
| GET | `/auth/me` | auth | `handle_get_current_user` |
| POST | `/auth/2fa/verify` | public | `handle_2fa_verify` |
| POST | `/auth/2fa/setup` | auth | `handle_2fa_setup` |
| POST | `/auth/recovery-code` | auth | `handle_generate_recovery_code` (400 if one exists unless `?regenerate=true`) |
| POST | `/auth/verify-recovery-code` | public | `handle_verify_recovery_code` — `{ username, code }` |
| POST | `/auth/reset-password-recovery` | public | `handle_reset_password_recovery` — code is not consumed |
| GET | `/auth/setup-status` | public | `handle_setup_status` — `is_first_run` (DB-derived), `is_setup_valid`, `hardware_changed`, `needs_recovery`, `server_machine_id`, `sudo_available` |
| GET | `/auth/check-username` | public (local network only) | `handle_check_username` |
| POST | `/auth/setup` | public (local network only) | `handle_setup` — accepts optional `totp_secret` + `totp_code` (enrolled at the wizard's password step); response may include `totp_enabled` + `backup_codes`. Refused once an admin exists |
| POST | `/auth/setup/verify-totp` | public (local network only, first-run only) | `handle_setup_verify_totp` — stateless pre-creation check of a client-generated TOTP secret: `{ secret, code }` → `{ valid }` |
| POST | `/auth/setup/rebaseline` | admin | `handle_setup_rebaseline` — re-baseline the hardware fingerprint after a confirmed hardware change |

The three unauthenticated bootstrap routes (`/auth/setup`,
`/auth/setup/verify-totp`, `/auth/check-username`) accept only loopback /
private / link-local peers (`middleware::setup_guard`); `HOMEVAULT_SETUP_ALLOW_REMOTE=1`
lifts the restriction. `/auth/setup-status` is exempt.

Login returns a session token (stored as `session-token` in localStorage) and
sets an `hv_session` cookie read only by the per-app reverse proxy.
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
| PUT | `/users/{username}/password` | admin | `handle_user_update_password` |
| PUT | `/users/{username}/role` | admin | `handle_user_update_role` |
| PUT | `/users/{username}/locked` | admin | `handle_user_set_locked` |
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
| GET | `/storage/raid/devices` | auth | `handle_raid_available_devices` — candidate physical disks for the RAID creation wizard |
| POST | `/storage/raid` | admin | `handle_raid_create` — creates one virtual disk (mdadm / ZFS / Btrfs by filesystem) |
| GET | `/storage/virtual-disks` | auth | `handle_virtual_disks_list` — **all** virtual disks (mdadm + ZFS + Btrfs) |
| DELETE | `/storage/virtual-disks/{name}` | admin | `handle_virtual_disk_delete` — dispatches mdadm vs ZFS/Btrfs |
| POST | `/storage/virtual-disks/{name}/scrub` | admin | `handle_virtual_disk_scrub` |
| GET | `/storage/raid` | auth | `handle_raid_arrays_list` — legacy; Storage's Virtual-disks tab is preferred |
| DELETE | `/storage/raid/{name}` | admin | `handle_raid_delete` — legacy |
| POST | `/storage/raid/{name}/rebuild` | admin | `handle_raid_rebuild` — reused by Storage's Rebuild button |
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
| GET/PUT | `/shares/smb\|nfs\|ftp\|webdav/config` | admin | service config |
| POST | `/shares/{share_type}/start\|stop\|restart` | admin | service control |
| GET/POST/DELETE | `/shares/{name}/{share_type}/permissions` | admin | per-user/group ACL |
| POST | `/shares/{name}/{share_type}/permissions/sync` | admin | re-apply ACLs |
| GET | `/users/{username}/permissions` | admin | `handle_user_permissions` |
| GET | `/shares/access` | admin | `handle_list_access` — every per-member resource grant (optional `?member=`) |
| GET | `/shares/access/mine` | auth | `handle_my_access` — the caller's own effective access (`all: true` for admins) |
| POST | `/shares/access` | admin | `handle_grant_access` — `{member, kind, resource, permission?}` |
| DELETE | `/shares/access` | admin | `handle_revoke_access` — `?member=&kind=&resource=` |

`/shares/access` is the **fine-grained per-member** layer (apps / folders /
shares), separate from the POSIX-ACL share permissions above. `kind` ∈ {`app`,
`folder`, `share`}, `permission` ∈ {`read`, `write`, `full`}. See
[Users](./guides/users.md) for the server-side filter contract.

`share_type` ∈ {`smb`, `nfs`, `ftp`, `webdav`}. Permission grant applies the real
POSIX ACL **before** the DB write; revoke deletes the DB row even if the ACL
removal fails (see `src/permissions.rs`).

## Network — `/api/network/*` (router in `src/handlers/network.rs`)

Read endpoints `auth`; config/state/WiFi mutations `admin`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/network/interfaces` | auth |
| GET | `/network/interfaces/{name}` | auth |
| GET | `/network/interfaces/{name}/config` | auth |
| PUT | `/network/interfaces/{name}/config` | admin |
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
| PUT | `/docker/containers/{name}/start\|stop\|restart` | admin |
| GET | `/docker/containers/{name}/logs` | admin |
| POST | `/docker/containers` | admin |
| DELETE | `/docker/containers/{name}` | admin |
| POST | `/docker/images/pull` | admin |
| DELETE | `/docker/images/{image_id}` | admin |
| GET | `/docker/hub/search` | admin |
| GET/POST/DELETE | `/docker/networks` (+ `/{name}`, `/prune`, `/connect`, `/disconnect`) | admin |
| GET/POST/PUT/DELETE | `/docker/compose/projects` (+ `/{name}/config\|start\|stop\|restart\|services`) | admin |
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

**App catalog (2026-09-03 overhaul):** `src/docker_catalog.rs::CATALOG` is gone.
Apps are packages — a `homevault-app.toml` manifest + a `docker-compose.yml`
template — from the embedded `catalog/` dir plus admin-added community stores.
Wire ids are namespaced `<store>:<app>`; the install key (compose project + data
dir) is the **bare** app id for the built-in `homevault` store, `<store>-<app>`
otherwise. Each installed app gets a dedicated reverse-proxy listener
(47000–47999, `src/app_proxy`) with an optional `hv_session`-cookie auth gate;
the app's own published port stays bound to `0.0.0.0` on purpose (the proxy is
an extra entry point, not a lockdown). See [Docker](./guides/docker.md).

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

Every handler in `src/handlers/syncthing.rs` calls a local `require_admin` —
`status`, `config`, `devices`, `folders`, `connections`, `errors` (GET);
`restart`, `pause`, `resume`, `scan` (POST). This closes a previously-real gap
where none of these handlers validated a session at all; see
[Syncthing](./guides/syncthing.md) for what was found and fixed.

## Downloader — `downloader/*` (`aria2`, all `admin`)

Every Downloader endpoint calls `require_admin_request`, so **all** are admin-only
(`status`, `global-stat`, `tasks`, `task/{gid}`, `start`, `stop`, `add`, `pause`,
`unpause`, `remove`). There are no read-only `auth` endpoints here.

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
never created or was already removed. In a `ProgressEvent`, `job_type` is a
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

`tools`, `devices` (`GET`); `ddrescue/clone`, `ddrescue/status/{log}`,
`testdisk/analyze`, `testdisk/rebuild`, `photorec/recover`, `mdadm/diagnose`,
`mdadm/repair` (`POST`). Implemented in `src/handlers/rescue.rs` + `src/rescue/`.

## VPN — `/vpn/*` (all `admin`)

Every VPN endpoint requires an **admin** session — the whole surface is
privileged (it writes `/etc` and brings up network interfaces as root), so there
are no read-only `auth` endpoints here. Capabilities, connections, WireGuard
(`/wireguard`, `/wireguard/keygen`, `/{name}/start|stop`, delete), OpenVPN
(`/openvpn`, `}/{name}/start|stop`, delete). Router in `src/handlers/vpn.rs`.

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

**No API.** The upload feature was removed 2026-09-04. The 15 bundled wallpapers
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

## Health endpoint detail

`GET /health` (`handle_health_endpoint`, `src/web/mod.rs:222`) requires a
valid session, runs `HealthService::run_all_checks()`, and reads CPU/memory
from `/proc/stat` and `/proc/meminfo`. Returns
`{ success, data: { checks[], overall_status, cpu_usage, memory_usage } }` —
note the field names are `checks`/`overall_status`, not `metrics`/`overall`.
See [Health](./guides/health.md).

## Related docs

- [Backend Structure](./BACKEND.md) — where each handler lives.
- [Frontend Structure](./FRONTEND.md) — `api.ts` client methods.
- Per-area guides under [guides/](./guides/).
