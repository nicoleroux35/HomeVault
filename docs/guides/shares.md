# Shares (SMB / NFS / FTP / SFTP / WebDAV + ACLs)

**Purpose:** Network file-share management for five protocols plus per-user/group
POSIX-ACL permissions.

## Backend

Handlers in `src/handlers/shares.rs` and `src/handlers/permissions.rs`. ACL logic
in `src/permissions.rs` (`PermissionService`). Share resolution
`SharesService::get_share(...).path` (must not be a hardcoded `/srv/{name}` guess).

SFTP is architecturally distinct from the other four: it's the SSH file-transfer
protocol, not an FTP variant, so it's served by a second, dedicated `sshd`
process (`homevault-sftp.service`, own port, own config under
`/etc/homevault-sftp/`) rather than by any of the SMB/NFS/FTP/WebDAV daemons —
never the host's real system sshd. Logic lives in `src/sftp.rs`
(`SftpJailService`): per-user chroot jails under `/var/lib/homevault-sftp/jails/`
built from bind mounts of exactly the shares a user has been granted (resolved
from the *same* ACL grants as every other share type — no separate
access-control list), key-only auth (`AuthorizedKeysFile` under
`/var/lib/homevault-sftp/keys/`, `PasswordAuthentication no`), and
`ensure_linux_user` provisioning a real, shell-less (`nologin`) Linux account
on first grant, since regular HomeVault users are DB-only otherwise.

### WebDAV — in-process Rust server (2026-10-14)

WebDAV is **not** a system daemon: it is an axum listener owned by HomeVault
(`src/webdav/`), built on the `dav-server` crate (hyper 1.x, same stack as the
app proxy). The previous implementation generated a Python
`/usr/local/bin/homevault-webdav.py` + a root-owned `homevault-webdav.service`
via `systemctl`; that is gone. `WEBDAV` is a process-wide manager
(`src/webdav/mod.rs`, mirrors `APP_PROXY`) with `start`/`stop`/`reload`;
`reload_if_running` rebuilds the listener after a share change.

- **Filesystem** — `src/webdav/vfs.rs` (`WebdavFs`) implements
  `GuardedFileSystem<WebdavCreds>`. The first URL segment is a share **name**
  (never a path); each request is delegated to a `LocalFs` rooted at that
  share's root, so `..` / symlinks can never leave the share. A cross-share
  `COPY`/`MOVE` is refused. Path mapping is in `src/webdav/fs.rs` (pure,
  unit-tested).
- **Auth** — `src/webdav/serve.rs` reads `Authorization: Basic`, verifies the
  credentials through `AuthService::authenticate` (real HomeVault accounts,
  lockout included) and answers 401 with the standard challenge otherwise.
  `require_auth: false` is the only open mode, and it is opt-in.
- **Per-member grants** — the `share` kind of `resource_shares` is enforced:
  a non-admin caller only reaches the shares named in its grants
  (`ResourceShareRepository::list_for_member`). This is the first consumer of
  the `can_access` layer for shares.
- **Config honored** — `port`, `path_prefix` (mount point), `require_auth`,
  `require_ssl` (reuses the HomeVault TLS certificate; refuses to start when
  TLS is required but not enabled), `locking` (`MemLs` vs `FakeLs`),
  `allowed_methods` (`DavMethodSet`). The default config is closed:
  `WebdavConfig::default()` is port 8082 + `require_auth: true` (the old
  derived default was port 0 + auth off).
- **Lifecycle** — the server runs iff at least one enabled WebDAV share
  exists; `POST /shares/webdav/start|stop|restart` drives `WEBDAV`. Startup
  also calls `cleanup_legacy_service()` to disable and remove the old
  systemd unit + Python script on an upgraded host. mDNS uses
  `crate::webdav::running_port()`, not `unit_active`.

### Config-file injection hardening (2026-10-21)

Every share value that reaches a root-owned config file is now validated
**before** it is formatted into that file, by pure validators in
`src/shares.rs` (`validate_share_name`, `validate_share_comment`,
`validate_share_path`, `validate_nfs_options`, `validate_smb_config`,
`validate_ftp_config`). They run in `SharesService::create_share` /
`update_share` / `delete_share` and in `save_smb_config` / `save_ftp_config`,
i.e. before any `sudo tee`, `systemctl`, `exportfs` or `useradd`, and the
handlers surface them as a `400` naming the offending field.

| Field | Accepted | Why |
|-------|----------|-----|
| `name` | ASCII letters, digits, `-`, `_`, `.`; ≤ 64 chars; no leading `.`, no `..`, not `global`/`homes`/`printers`/`admin$` | becomes an `smb.conf` `[section]` header, an `/etc/homevault/*.json` key and a jail directory (`jail/<name>`) |
| `comment` | one line (no control chars except tab), ≤ 200 chars | `comment = …` is one `smb.conf` line — a newline forged extra directives |
| `welcome_message` (FTP) | one line, ≤ 120 chars | `ftpd_banner=…` is one `vsftpd.conf` line |
| `path` | absolute, no whitespace/control/`( ) , # ;`, no `..` | first column of `/etc/exports` and of the SFTP `/etc/fstab` bind entry — a space shifts columns, `)`/`#` rewrite the option list |
| `options` (NFS) | one token each: no whitespace/control/`( ) , # " '`, ≤ 16 entries | written as `path *(opt1,opt2)`; a `)` closed the client list and fabricated extra exports |
| SMB global | `workgroup` one line ≤ 15, `security_mode` ∈ {user, share, domain, server, ads}, `protocol_version` alnum/`_`/`.`, masks 3-4 octal digits | `[global]` directives are one line each |
| FTP ports | `port`/passive bounds non-zero u16, `passive_ports` = `min` or `min-max` with min < max, `max_clients` ≥ 1 | parsed then formatted into `vsftpd.conf` |

`PathGuard` still applies downstream (it guards the *filesystem* operations of
the share's own content); these validators guard the *config files* — a share
whose path is outside the allowed roots is rejected later, not here.

### NFS exports are rewritten, not appended around (2026-10-21)

`/etc/exports` is now handled through one pure parser/generator pair
(`parse_nfs_export_line`, `nfs_export_line`), so every mutation rewrites
exactly one line:

- **disable** prefixes the line with `# homevault-nfs-disabled:<name>` and
  **enable** strips it again, followed by a best-effort `exportfs -ra`. The
  old toggle ran `exportfs -o rw /<name>` / `exportfs -u /<name>` — a
  no-op/wrong-shape call that changed only the in-memory export table, so a
  disabled share silently came back on the next reload or reboot. Tagged lines
  are parsed back as *disabled* shares, so the UI keeps listing them.
- **delete** and **update** resolve the single matching line by parsed share
  name and rewrite only that line — the old code removed/replaced *every* line
  whose path basename matched, so two exports `/data/media` and `/backup/media`
  silently deleted each other. A name that matches several exports is now an
  explicit `400` listing the colliding paths instead of a wrong-line edit.

### Blocking work is off the async runtime (2026-10-21)

`SharesService` is entirely synchronous (file IO + `sudo`
`tee`/`systemctl`/`exportfs`) and `SftpJailService::rebuild_jails` shells out
per grant. Handlers no longer call either directly:

- `run_share_op(op, closure)` in `src/handlers/shares.rs` runs every service
  call on `spawn_blocking` under a 30 s `tokio::time::timeout`.
- `rebuild_jails` is split in two phases — resolve the desired state from the
  DB (async), then reconcile accounts/mounts/fstab on `spawn_blocking` — under
  a 120 s timeout. `list_grantees` and `set_authorized_key` are wrapped the
  same way.
- A hung `mount --bind`/`systemctl` now returns a clean error naming the
  operation and its timeout instead of pinning a runtime worker forever.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /shares` | admin | list shares |
| POST | `/shares` | admin | create (`share_type` incl. `sftp`) |
| `GET /shares/{name}/{share_type}` | admin | get |
| `PUT /shares/{name}/{share_type}` | admin | update |
| `PUT /shares/{name}/{share_type}/toggle/{enabled}` | admin | enable/disable |
| `DELETE /shares/{name}/{share_type}` | admin | delete |
| `GET/PUT /shares/smb\|nfs\|ftp\|webdav\|sftp/config` | admin | service config |
| `POST /shares/{share_type}/start\|stop\|restart` | admin | service control |
| `GET /shares/sftp/users` | admin | list users with SFTP access + key status |
| `PUT /shares/sftp/users/key` | admin | set a user's SFTP public key (`{ username, public_key }`) |
| `POST /shares/sftp/rebuild` | admin | reconcile SFTP jails against current ACL grants |
| `GET/POST /shares/{name}/{share_type}/permissions` | admin | list/grant ACL |
| `DELETE /shares/{name}/{share_type}/permissions` | admin | revoke ACL (`{ grantee_type, grantee_name }`) |
| `POST /shares/{name}/{share_type}/permissions/sync` | admin | re-apply ACLs |
| `GET /users/{username}/permissions` | admin | user's permissions |

`share_type` ∈ {`smb`, `nfs`, `ftp`, `webdav`, `sftp`}.

**SFTP grant order matters:** a share-permission grant applies a real `setfacl`
entry for the grantee, which requires that user to already exist as a real
Linux account — so a user's SFTP key must be set (`PUT /shares/sftp/users/key`,
which provisions the account) *before* granting them a share, not after.
Granting first fails with a `setfacl` error.

## Deadlines on every host tool (2026-10-24, ROADMAP X.2)

The handlers already wrap a share operation in `run_share_op`
(`spawn_blocking` + `tokio::time::timeout`), but that outer timeout only stops
**waiting** for the `spawn_blocking` future — it does not kill the `sudo
systemctl`/`exportfs`/`tee` child, which keeps the blocking thread pinned. Every
host command of this module now carries its own deadline, so the child is killed
at the ceiling:

| Call | Deadline | Why |
|---|---|---|
| `sudo tee`/`sudo tee -a`, `sudo mkdir -p` | `SHARES_WRITE_TIMEOUT` 30 s | a config write; `run_timed_with_stdin` feeds the body and closes stdin |
| `systemctl start/stop/restart/reload`, `exportfs -ra` | `SHARES_SERVICE_TIMEOUT` 60 s | a real service start can wait on its dependencies |

`run_command` keeps a non-zero exit as *data* (the callers either surface the
stderr or only care that the reload ran). A source guard
(`test_no_subprocess_of_this_module_runs_without_a_deadline`) fails if a bare
`.output()`/`.spawn()`/`.status()`/`wait_with_output()` comes back.

## Frontend

- `frontend/src/routes/Shares.svelte` (uses `$state(...)` for all reactive
  fields — plain `let` would not trigger UI updates). The SFTP tab includes an
  "Add SFTP user" free-text form (not restricted to existing grantees) so a
  user's key can be set before any share grant exists — see grant-order note
  above.
- `frontend/src/lib/services/api.ts`: `getSharePermissions`,
  `grantSharePermission`, `revokeSharePermission`, `syncSharePermissions`, and
  the generic `delete(..., body)` used by revoke. SFTP config/user endpoints go
  through the generic `get`/`put`/`post`, no dedicated methods.
- WebDAV access URL is assigned in a live code path (a past bug had it inside an
  `{#if false}` block).

## Invariants

- **Every value written into a root-owned config file is validated first.**
  `validate_share_name` / `validate_share_comment` / `validate_share_path` /
  `validate_nfs_options` / `validate_smb_config` / `validate_ftp_config` run
  inside `SharesService` before any `sudo tee`, so *any* caller (handlers,
  cache-share sync, ransomware auto-disable) is covered — never format an
  unvalidated share field into `smb.conf`, `/etc/exports`, `vsftpd.conf` or
  `/etc/fstab`. The SFTP jail code reuses `validate_share_path` /
  `validate_share_name` for its fstab line and for grantees' usernames.
- **`/etc/exports` is parsed and re-serialised through one pure pair**
  (`parse_nfs_export_line` / `nfs_export_line`); a mutation rewrites exactly
  one line and a name matching several exports is a `400`, never a
  multiple-line rewrite. Disabled exports stay in the file behind the
  `# homevault-nfs-disabled:<name>` tag so the state survives reboots.
- **No `SharesService`/`SftpJailService` call runs inline in an async
  handler.** They go through `run_share_op`/`run_jail_rebuild`
  (`spawn_blocking` + timeout). `SftpJailService::rebuild_jails` keeps its
  async DB resolution phase separate from its blocking reconciliation phase —
  keep that split when touching it.
- **Every host command carries a deadline** (see above): `run_command` /
  `run_timed_with_stdin` are the only allowed shapes. The handler's outer
  `tokio::time::timeout` stops waiting but does **not** kill the child, so a
  bare `.output()` in this module is a regression the source guard test catches.
- **WebDAV has no systemd unit.** Never reintroduce `systemctl` calls for the
  `webdav` protocol, a generated `homevault-webdav.py`, or a root-owned DAV
  process. `SharesService::start/stop/restart_service(Webdav)` returns an
  error on purpose; the routes go through `crate::webdav::WEBDAV`. The share
  table and per-member grants are the boundary — a URL's first segment is a
  share *name*, and `WebdavFs` re-checks the caller's grant on every
  operation, including both sides of `COPY`/`MOVE`.
- **All** Shares endpoints require `require_admin` (a past bug accepted any
  non-empty token on `handle_shares_list`/`handle_share_get`; they now use
  `require_admin`, like every other handler in `src/handlers/shares.rs`).
- `grantSharePermission` applies the real POSIX ACL **before** writing the DB row,
  so a failed ACL application does not leave a phantom "granted" permission.
- `revokeSharePermission` deletes the DB row even if the filesystem ACL removal
  fails (so a stale permission can still be cleaned from the UI).
- ~~**Known bugs in the ACL layer (not SFTP-specific, reported not fixed):** a
  grant with `recursive: true` fails outright (`setfacl` error from
  `set_default_acl`) and is never persisted; the "rw" permission option grants
  no execute/traverse bit, which makes it unusable on a directory (every share
  is one) — use "rwx" for real read-write directory access.~~
  **Both are already fixed in the code** (stale note, re-read on 2026-10-12;
  the ACL half still needs a live-host confirmation before it can be called
  verified — see `TESTING.md` → "Remaining to test as a priority"):
  `AclService::set_default_acl` (`src/acl.rs`) now passes `-d -m` in that
  order — `setfacl`'s parser takes the element after `-m` as its value
  regardless of a leading dash, so the old `-m -d spec` fed it the literal
  `-d` as the ACL spec; and `PermissionService::apply_permission_to_path`
  (`src/permissions.rs`) forces the execute bit on when the share path is a
  directory, in the ACL actually written to disk only — the persisted
  `permission` string (what the UI shows) is untouched.
- `handle_user_delete` (`src/handlers/users.rs`) calls
  `SftpJailService::purge_user` directly rather than via `rebuild_jails`,
  because `rebuild_jails` only reconciles jails for users that still exist.
  (The companion half of the old note — that a deleted user's direct share
  permissions were "logged as revoked but never actually are" — is **fixed as
  of 2026-10-12**: `handle_user_delete` now calls
  `PermissionService::revoke_all_user_permissions`, which removes the real
  ACLs and the `share_permissions` rows. See [Users](./users.md) →
  Invariants.)
- `SftpJailService::ensure_linux_user` clears the locked-password flag
  `useradd` leaves on a fresh account (OpenSSH rejects a locked account even
  for pubkey-only auth) — with a random, discarded password, never for an
  account that already existed.
- `authorized_keys` files are `0644`, not `0600` — OpenSSH's unprivileged
  pre-auth worker reads them directly and can't open a `0600` file it doesn't
  own; a public key isn't a secret, so this is safe.
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Shares.svelte`, the permission api.ts methods, `src/handlers/shares.rs`,
  `src/handlers/permissions.rs`, `src/permissions.rs`, `src/sftp.rs` and the
  SFTP line in `handle_user_delete` are open to edits. The invariants above stay
  binding: a change keeps the Rust integration + Playwright E2E suites green.

## Related

- [Explorer](./explorer.md), [Storage](./storage.md), [API Contract](../API.md)
