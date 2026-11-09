# Changelog

All notable changes to HomeVault are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versioning
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Virtual-machine export / import, one portable archive (2026-11-09, P2).**
  DSM's OVA export, built on the native Incus primitive: a whole machine
  (configuration + disk) becomes one file that `incus import` restores here or
  on another HomeVault.
  - `POST /api/vm/{name}/export` (`{snapshot?, compressed?}`), `GET
    /api/vm/exports`, `POST /api/vm/import`, `GET /api/vm/exports/{file}`
    (streamed download) and `DELETE /api/vm/exports/{file}` — admin-only, the
    long operations tracked as `vm_export` / `vm_import` jobs;
  - archives live in `<DATA_DIRECTORY>/vm/exports`; the name is derived
    server-side from an already-validated machine name plus a timestamp and
    re-validated on every later use (no `/`, no `..`, `.tar`/`.tar.gz` only);
  - the sudoers grants pin the archive path to that folder for both commands,
    so a compromised argv cannot make `incus` read or write anywhere else;
  - the UI adds an *Export / import* dialog (export the selected machine, list
    the archives with their path, import with an optional new name, delete);
    without Incus the actions are disabled with the reason.
  Tests: 5 unit, 3 integration (no `incus` command is executed), VM E2E §11
  (written, not run).

- **iSCSI thin LUNs on ZFS, with snapshots and clones (2026-11-09, P2).** A
  shared disk can now be a **thin ZFS volume** instead of a sized file: created
  by HomeVault under `<pool>/homevault-iscsi/`, allocated on write, and
  snapshottable at any instant.
  - `kind: "zvol"` on `POST /api/storage/iscsi/backstores` (`{pool, size_bytes,
    sparse?}`) — the device path is derived, never supplied, and a non-ZFS pool
    is refused with the reason;
  - `GET/POST …/backstores/{id}/snapshots`, `DELETE …/snapshots/{snapshot}` and
    `POST …/backstores/{id}/clone` — the clone becomes a new backstore ready to
    export as its own LUN, sharing the snapshot's blocks until one side changes;
  - these endpoints only ever touch `/dev/zvol/<pool>/homevault-iscsi/<name>`,
    so they cannot snapshot or destroy a dataset HomeVault did not create; a
    zvol's kind, name and path are immutable in place (only its read-only flag
    changes), because converting an existing backstore would silently re-point
    an exported LUN;
  - deleting a zvol disk removes the volume and its snapshots (`zfs destroy -r`)
    only after the name is typed, like the file case;
  - sudoers grows by exactly `zfs create` and `zfs clone`.
  Tests: 6 unit (`src/zvol.rs`), 4 integration (no `zfs` command is executed),
  iSCSI E2E §7 (written, not run).

- **ZFS read cache and separate log (L2ARC / SLOG) (2026-11-09, P2).** The
  other half of DSM's SSD cache, next to HomeVault's existing write-cache tier:
  an L2ARC or SLOG vdev attached to an existing ZFS pool, from the Cache tab.
  - `GET/POST /api/zfs-cache` and `POST /api/zfs-cache/remove` (admin): list
    each pool's cache/log devices with their state and error counters, attach a
    free disk, detach one;
  - `zpool status -P` is parsed by a pure, unit-tested function (sections,
    device rows, mirrored log groups);
  - the server refuses a **spinning disk** (an L2ARC on an HDD makes the pool
    slower, a SLOG gains nothing), a device in use, a second vdev group, and a
    pool that is not ZFS — each with a sentence the UI shows verbatim;
  - the sudoers grant grows by exactly two subcommands (`zpool add`,
    `zpool remove`), and both operations are journaled.
  Tests: 8 unit, 3 integration (no `zpool` command is executed), Storage E2E
  §14 (written, not run).

- **Downloader: RSS subscriptions, per-task limits and automatic extraction
  (2026-11-09, P1).** The Download Station gap, filled with what the appliance
  already has (aria2 + the generic scheduled-task engine):
  - **RSS/Atom feeds** (`src/downloader_rss.rs`): subscribe by address, check a
    feed, queue **Download new**, or let a scheduled task do it
    (`service = downloader_rss`). The pure parser keeps only items with a
    downloadable URL (enclosure, else link; `http(s)`/magnet only), a bounded
    per-feed `seen` marker records what was *actually* queued, and a feed URL
    is refused when it points at loopback/RFC 1918 — the refresh must not
    become an SSRF primitive. Feeds live in `data/downloader_feeds.json` (0600).
  - **Per-task limits** on `/downloader/add`: `max_download_limit`,
    `max_upload_limit`, `seed_ratio`, validated (aria2 syntax) and sent in the
    task's own options.
  - **Automatic extraction** (off by default): a finished `.zip`/`.tar.*`
    archive is unpacked **in the folder that download wrote into**, from an
    allowlist that matches the installed `tar`/`unzip`; the archive is kept and
    each download is recorded once. A **Extract archives** button does the same
    on demand.
  Tests: 14 unit, 4 integration, Downloader E2E §15 (written, not run).

- **Network locations — CIFS/NFS remote mounts in the Explorer (2026-11-09, P1).**
  A share on another NAS or computer is now browsable like any folder, without
  a new daemon:
  - every mount lands on `/mnt/homevault-remote/<name>` where `name` is a
    validated lowercase slug — a request never names a target path, so a
    location can never be mounted over `/etc` or a pool;
  - the server builds the `mount` argv from a stored definition
    (`data/remote_mounts.json`); a CIFS password lives only in a **0600**
    credentials file and is passed as `credentials=<path>`, never on the
    command line, in a log line or in an API response (`has_credentials` only);
  - options are a per-protocol allowlist, `ro`/`rw` is its own field
    (read-only by default), and every command has a deadline that reports the
    tool's own stderr;
  - `enabled` locations are remounted at startup (a NAS that is off is a
    warning, never a boot failure), and **Reconnect all** re-runs the pass;
  - admin-only, and honest: a host without `mount.cifs`/`mount.nfs` answers
    `503` naming the tool and the panel says so instead of offering a button.
  `GET/POST /api/remote-mounts`, `PUT/DELETE /api/remote-mounts/{name}`,
  `POST …/mount|unmount`, `POST /api/remote-mounts/reconcile`; UI panel in the
  Explorer sidebar. Tests: 10 unit in `src/remote_mounts.rs` + 2 in
  `src/snapshots.rs`, 7 integration, Explorer E2E §19 (written, not run).

- **Btrfs file versions in the Explorer timeline (2026-11-09, P1).** Rewind
  listed ZFS snapshots only; it now reads the pool's
  `.snapshots/<share>/<name>/…` tree when the path's share lives on Btrfs, so
  a Btrfs pool gets the same per-file history. The Btrfs lookup only runs when
  ZFS found nothing, and the share/pool resolution respects segment boundaries
  (`/srv/photos` never matches `/srv/photos2`). Tests: 2 unit (written, not
  run).

- **Media integration: share binding and hardware transcoding (2026-11-09, P1).**
  The catalog's media apps (Jellyfin, Immich, Navidrome) can now use your files
  and your graphics card instead of an empty sandbox folder:
  - a manifest declares `share_mount` (`/media`, `/mnt/media`, `/music`) and
    the install dialog offers a **Library** selector over the configured
    shares; the server resolves the share itself, mounts it **read-only**, and
    refuses an app that does not declare one (`share_mount` is validated at
    parse: absolute, no `..`, `:` or whitespace);
  - **hardware transcoding** is an opt-in that needs the manifest's
    `gpu = true`, the request's `gpu: true` and a real `/dev/dri`; the GPU
    checkbox is disabled with the reason when the host has no render device;
  - `.install.toml` now records the settings, the share and the GPU choice, and
    updates render from them — which also fixes an update silently dropping
    the admin's declared environment values.
  Tests: 10 unit (share/GPU rendering, `replace_block`, manifest validation,
  marker round-trip), 3 integration, Docker E2E 15.1/15.3b (written, not run).

- **WireGuard remote-access UX (2026-11-09, P1).** Peer configs (and their QR
  codes, already shipped) now use a public endpoint resolved in order: a manual
  admin override, the configured Dynamic DNS hostname, then the detected public
  IP. The VPN card shows the value and its source and warns when it is unset or
  a private/LAN/CGNAT address, so a config that looks valid but only works
  indoors is called out before a device is created.
  `GET/PUT /api/vpn/wireguard/endpoint`; the override persists in
  `data/wg_endpoint.json`. Tests: 2 unit, 1 integration, VPN E2E 1.5 (written,
  not run).

- **Quota usage history, 90% alerts, and Journal export (2026-11-09, P1).**
  An hourly sampler stores transfer-quota usage in `transfer_quota_history`
  (migration 088, 90-day retention) and raises one system notification when a
  limit crosses 90% within a period (staying above never re-alerts; a reset is
  a fresh baseline). The Quotas screen opens a read-only history dialog per
  row, and `GET /transfer-quotas/history` serves the points. The Journal
  toolbar gained **Export CSV**, backed by `GET /journal/export` (same filters
  and privilege model as the screen, CSV or JSON, 5 000 entries). Tests:
  4 unit, 2 integration, E2E Quotas 4.4 + Journal 3.1b (written, not run).

- **Scheduled tasks can trigger existing subsystems (2026-11-09, P1).** The
  Tasks engine gained a fifth type, `service`, so a cron can trigger work that
  already has an implementation instead of a second scheduler: a Docker/catalog
  update pass, the trash-retention policy, a `VACUUM INTO` database snapshot,
  or a notification with a custom message. Restic/scrub keep their own
  schedule on purpose (one engine per job). Migration 087 rebuilds the task
  tables to widen the type CHECK while preserving run history. The create form
  shows a service picker (and a message field only for notifications). Tests:
  3 unit, 1 integration, Tasks E2E 3.5 (written, not run).

- **Scheduled Docker/catalog updates (2026-11-09, P1).** The Docker catalog tab
  gained an opt-in **Scheduled updates** disclosure. Off by default; the first
  tick after enabling records a baseline and waits one interval, so no
  container is restarted on a whim. A due run calls the same updater as "Update
  all", checks for a pending image/package update first, and only recreates
  apps that have one. A failed run advances the clock and records `last_error`
  instead of retrying every five minutes. Admin API:
  `GET/PUT /api/docker/updates/schedule`, `POST /api/docker/updates/run-now`.
  Settings persist in `data/docker_updates.json` (interval 6–168 h). Tests:
  5 unit in `src/docker_update_scheduler.rs`, 3 integration in
  `tests/docker_update_schedule_integration.rs`, Docker E2E 15.6 (written, not
  run).

- **Windows network discovery (2026-11-08, P1).** The SMB card gained a
  **Windows network discovery** toggle (on by default). It is applied by a new
  `homevault-wsdd.service` running `wsdd2`, which advertises only the NetBIOS
  name and workgroup — no share or file. The unit is sandboxed (`DynamicUser`,
  `NoNewPrivileges`, `ProtectSystem=strict`, a narrow address-family set);
  turning it off disables and removes it. `GET /api/shares/ws-discovery`
  reports available/enabled/running with a sentence, and the card says
  "wsdd2 is not installed" instead of hiding a broken switch. The installer
  ships `wsdd2` as an optional package. Tests: 6 unit in
  `src/ws_discovery.rs`, 1 in `src/shares.rs`, 1 integration in
  `tests/shares_integration.rs`, E2E Shares 4.9b (written, not run).

- **Non-destructive restore drill (2026-11-08, P2-9).** Settings → System →
  Database snapshots now has **Verify restore**: it copies the newest DB
  snapshot to a temporary directory, opens it read-only and runs SQLite's
  `PRAGMA integrity_check`, decodes the newest configuration archive with
  per-entry checksum verification (an encrypted archive needs its passphrase),
  and reports the newest restic job. The verdict is "Ready to restore" only
  when both the snapshot and the archive are fully verified; the report is
  saved to `data/restore-drill.json` and exposed by
  `GET/POST /api/settings/backup/restore-drill`. The drill never attaches the
  live database and never writes to the data directory. Tests:
  `src/restore_drill.rs` (4 unit), `tests/restore_drill_integration.rs` (5),
  Settings E2E 8.6 (written, not run). Docs:
  `docs/guides/disaster-recovery.md`.

- **Queryable audit trail (2026-11-08, P1-18).** Every application event
  written through `events::log_event` / `log_event_warn` is now also persisted
  in the `audit_log` table (migration 086) through a bounded background
  channel, so logging never waits on SQLite. Authentication events
  (`auth.login`, `auth.login.trusted`, `auth.login.failed`, `auth.logout`)
  carry structured actor/action/client-IP columns; the plain-English sentence
  remains the canonical message, and no file content or secret is stored.
  Admins can query it with `GET /api/audit` (`from`, `to`, `severity`,
  `actor`, `action`, `q`, `limit`, `offset`) and download CSV/JSON from
  `GET /api/audit/export`; the Journal screen exposes the same table as the
  `audit` source. Rows older than 90 days are pruned at startup and every six
  hours. Tests: unit in `src/audit.rs`, integration in
  `tests/audit_integration.rs`, E2E 2.4 in the Journal spec (written, not run).
  Docs: `docs/guides/journal.md`, `docs/API.md`, `PRIVACY.md`.

- **Signed, atomic updates with automatic rollback (2026-11-08, P2-8).** The
  installer now verifies a signed release manifest (Ed25519 on OpenSSL 3,
  RSA/ECDSA SHA-256 otherwise) before downloading anything, enforces its
  `version`/`arch`/`sha256`/`size`, extracts to a staging directory and runs
  `homevault --version` on the staged binary before an atomic rename — the live
  executable is never overwritten in place. The previous build is kept as
  `homevault.previous`; if `/api/auth/setup-status` does not answer within 20 s
  the installer rolls back automatically, keeps the failed build as
  `homevault.failed` and records the outcome. Results are written to
  `data/updates/last-result.json`, exposed by `GET /settings/version` as
  `last_update` and shown in Settings. New `--verify-manifest` mode (no root)
  and `HV_SIGNING_KEY` signing during `--package`. The binary gained
  `--version`. Tests are in `installer/tests/update-engine.test.sh`; docs in
  `docs/guides/settings.md` → "System → Signed, atomic updates".

- **Explorer authorization, application gate and sharing (2026-11-08,
  P0–P3).** Every mutating Explorer route now checks the caller's
  `resource_shares` level, not just `PathGuard`: `write` for
  create/upload/rename/trash/save/compress/extract, `read` on the source +
  `write` at the destination for a copy, `write` on both for a move, and
  `full` for `chmod`. `preview`, `thumbnail` and thumbnail tickets honour
  `disable_download`; `list` honours `disable_browsing`. Listings and
  `/explorer/roots` return the effective `permission` and restrictions, and
  the UI disables the actions the server would refuse (read-only banner).
  A member whose `app:explorer` grant is `deny` is refused the entire
  `/api/explorer/*` surface by a new middleware, with an Available/Blocked
  switch in the Users card. Share links now accept folders (tar.gz snapshot
  built at creation), can target named accounts/groups on a `signed_in` link,
  require a `read` grant, honour `disable_download`, and are re-checked
  against the owner's grant on every download. Public **file requests**
  (upload links for a folder, password/expiry/cap, 2 GiB per file, plain HTML
  page) were added. The previous behaviour (write operations reachable from a
  read grant, and a share-link gate that trusted `PathGuard` alone) is gone.
  See `docs/guides/explorer.md` → "Access model and enforcement matrix" and
  "File requests".

- **Shares hardening against DSM 7.4 (2026-11-08, P0/P1/P2).** The share
  subsystem now provisions the Unix/Samba identity of a new account
  (`HV_HOST_SYNC=1`), generates SMB `valid users` from the ACL grants so a
  share is never open to every Samba account, and enforces `resource_shares`
  (including `deny` and `read` vs `write`) in WebDAV while scoping the
  non-admin Explorer to its home and grants. FTP entries are real per-user
  jails (bind mounts + `user_config_dir`), logical shares group protocol
  bindings under one name/path, NFS rules are per client, recursive ACLs are
  genuinely recursive and bounded, and a permission matrix/CSV report is
  available. P2 adds per-share advanced restrictions, `shadow_copy2` Previous
  Versions, SMB ADS/Kerberos configuration + join/leave, and NFS `sec=…`.
  See `docs/guides/shares.md` → "Drop-in hardening — P0/P1/P2".

- **Local DNS server (2026-11-05, phase C1).** Settings → Security → **Local
  DNS server** lets the appliance answer the names of your own network instead
  of leaving that to the router — the counterpart of the DHCP server it shares
  a `dnsmasq` with. Three kinds of answer: **local names** (an address for
  `nas.lan`, or an alias of another local name), **answered domains** (every
  name under `.corp.lan` gets one address and is never forwarded) and the
  **upstream servers** that handle everything else. One switch tells the devices
  that take their address from the appliance to resolve their names here too.
  Nothing leaks outward: single names are never forwarded, reverse lookups of
  private ranges never leave the network, the host's own resolver is ignored in
  favour of the servers you list, and a loopback upstream is refused. Names are
  grammar-checked (never escaped, so no line can inject a directive), an alias
  whose target the appliance does not serve is refused instead of being silently
  ignored, and turning the server on is confirmed. Since the address server and
  the name server are one program, the DHCP card now says so: stopping it is
  refused while the name server is on, and turning DHCP off no longer silences
  a running name server. See `docs/guides/local-dns.md`.

- **See, verify and remember every transfer in the File Explorer (2026-10-31,
  QW-13).** A comparison against DSM 7's File Station found HomeVault ahead on
  the fundamentals (chunked resumable uploads, a SHA-256 download digest, the
  reflink copy cascade, quota refunds) but behind on *visibility*: one
  percentage per job, no per-transfer cancel, no way to ask for a checksum, and
  nothing recorded once a transfer finished. Five parts close that gap, all
  documented in `docs/guides/explorer.md` → "Transfer tray, enriched progress
  and history".

  **Transfer tray (HV-UX-01).** A toolbar button with a running-count badge
  opens three zones — uploads, downloads, copies — with a progress bar per row,
  and a cancel button that really stops the work (the session abort for an
  upload, the progress manager's cancellation hook for a job). A collapsible
  section lists the last finished transfers. New `TransferTray.svelte`,
  `GET /api/explorer/transfers/active`.

  **Detailed progress (HV-UX-02).** Progress events, the activity feed and the
  tray now carry `bytes_processed`, `bytes_total`, `speed_bps`,
  `eta_seconds` and the copy `method`, with the speed taken over a rolling
  five-second window and the ETA derived from it. The client formats them
  (`45.3 MB / 128.7 MB · 12.0 MB/s · 2 min left`) instead of parsing a
  server-rendered sentence.

  **Uploads survive a restart (HV-UX-03).** Resumable upload sessions moved from
  one in-memory map to the `upload_sessions` table (migration 074). A chunk that
  arrives after a restart is rehydrated from the database and resumes at the
  stored offset; the 30-minute TTL is idle-based, and the concurrency caps are
  counted in rows so a restart cannot reset them.

  **Checksums on demand (HV-INT-01).** `POST /api/explorer/checksum` computes
  SHA-256, SHA-512 or BLAKE3 for any file — inline up to 8 MiB, as a tracked job
  above it — and `compare_with` answers whether a second file is identical. MD5
  is refused rather than offered. The digest of an asynchronous run is read back
  from `GET /api/explorer/checksum/result`.

  **Transfer history (HV-ADV-02).** `transfer_history` (migration 073) records
  one row per finished upload, download, copy, move and checksum: bytes,
  duration, how the copy was made, whether the bytes were verified, and the
  outcome. Downloads are recorded when the body ends (complete) or is dropped
  (cancelled), not when they start. `GET /api/explorer/transfers/history` scopes
  rows to their owner (an administrator sees every user's), refuses an unknown
  filter instead of ignoring it, and keeps 90 days.

  Deviations: no per-task pause/resume (pausing a server-side stream needs a
  different data path than these copies; cancel plus resume-from-offset covers
  the need), and the broadcast-copy idea from the same comparison was not taken.

- **Back up a Mac to a share, and get it back when a disk dies (2026-10-31,
  Phase 2 — protocol and backup interoperability).** Phase 2 is about the
  copies your data lives on, and every item is a working path rather than a
  checkbox.

  **Time Machine over SMB (P2.1).** A share can now be a macOS Time Machine
  destination. Turning it on writes one managed block into that share's
  `smb.conf` section — `fruit`/`streams_xattr` plus the recipe `vfs_fruit`
  documents (`fruit:time machine = yes`, `fruit:metadata = stream`,
  `fruit:posix_rename`, the `_adfile` wipe/delete options) and an optional
  maximum size, because a backup that silently fills the pool is worse than no
  ceiling at all — and publishes a `_adisk._tcp` advertisement for **that**
  share in its own Bonjour service group, with a `adVU` UUID derived from host
  and share so macOS keeps recognising the same volume across restarts. AFP is
  not used: macOS backs up over SMB today. The recycle bin and Time Machine
  share one managed block, because `vfs objects` can only be set once per share
  section, and a legacy recycle-only block upgrades in place.

  **Rsync server (P2.2).** Shares → **Rsync**: an opt-in daemon of HomeVault's
  own (`homevault-rsync.service`, its own port and config, never the host's
  `rsyncd`), with one authenticated module per share. A module can only serve a
  folder that is already a HomeVault share, re-checked against the live
  inventory on every apply, so the screen can never expose another part of the
  machine; a module with no allowed accounts is refused outright, because an
  anonymous rsync export of a home directory is exactly the accident this must
  not make possible. Each account gets its own rsync password — stored `0600`,
  written through a pipe, never returned by the API, never shown again. The
  daemon chroots by default and does not wait for reverse DNS. The screen also
  spells out the path most people actually want: rsync over SSH, using the SFTP
  account and key that already exist.

  **Backup destination (P2.3).** HomeVault can now *receive* backups: a
  **Backup destination** app turns on a dedicated `rest-server` unit
  (`--append-only --private-repos`, its own port and htpasswd file) and gives
  each sending machine a tenant with a token, a private repository and a quota.
  Append-only is the whole point — a compromised source cannot `forget` or
  `prune` the history it depends on, so ransomware on a laptop cannot reach
  round and delete its own backups. The token and repository password are
  displayed exactly once and never again (SHA-256 for the token, the appliance
  key for the password); over quota, the tenant is removed from the htpasswd
  file so every request is refused until an admin raises the limit; a tenant can
  ask for a daily `restic check` and run one on demand. Deleting a tenant
  removes access and leaves the repository on disk.

  **Snapshot replication (P2.4).** A **Replication** app pushes ZFS/Btrfs
  snapshots to another dataset — local, or another machine over SSH. Each run
  creates its own `hv-repl-<job>-<timestamp>` snapshot, sends the increment from
  the last successful run, resumes an interrupted stream instead of restarting
  it, and prunes **only** the copies it made on the destination: the source is
  never pruned and a snapshot you made by hand is never touched. An optional
  `pv` throttle caps the bandwidth, and when `pv` is missing the job says the
  cap was not applied instead of pretending. Before anything moves, **Preview**
  shows the exact command the next run would execute. Every stage is an argv
  vector, so no credential and no raw argument list reaches a log or a shell.

  **Immutable snapshots (P2.5).** A snapshot can now be **locked** for a number
  of days, and the lock is real: a row the appliance consults *and* a
  filesystem hold (`zfs hold homevault-lock` / a read-only Btrfs subvolume), so
  `zfs destroy` itself fails while the lock is active. Retention never overrides
  a lock — the prune skips a locked snapshot and reports how many it kept — and
  a lapsed lock is not silently treated as released: the release loop runs the
  release first and only then marks the row, so an unreachable pool keeps the
  snapshot protected. Releasing before the horizon is break-glass: the dialog
  will not enable its button until you type the phrase the server returned, and
  the release is journaled as `BREAK-GLASS:` with who, when and why. Rows are
  never deleted, so the history of who unlocked what stays readable.

  Phase 2 also adds the runbook it needs: a **disaster recovery** guide that
  says which copy answers which question (snapshots undo a mistake, replication
  survives a dead disk, the destination holds another machine's backups,
  configuration backup rebuilds the appliance but not the data), walks a
  bare-metal rebuild on new hardware, and gives the quarterly rehearsal worth
  doing before you need it.

  Guides: `shares.md` (Time Machine), `mdns.md` (`_adisk._tcp`), `snapshots.md`
  (locks), and the new `rsync.md`, `backup-target.md`, `replication.md` and
  `disaster-recovery.md`. Migrations `068` (locks) and `069` (replication),
  `070` (backup tenants). Tests: **+68 integration** (`time_machine_integration`
  8, `rsync_integration` 7, `replication_integration` 7,
  `backup_target_integration` 8, `snapshot_locks_integration` 7, plus the unit
  suites in `rsync_server.rs`, `backup_target.rs`, `replication.rs`,
  `snapshot_locks.rs` and their schedulers), and **43 new E2E** across
  `tests/e2e/{shares/time-machine,shares/rsync,backup/destinations,backup/replication,storage/snapshot-locks}.spec.ts`.

- **Sign in with your finger, not a password (2026-10-31, Phase 1 P1.3 —
  passkeys).** An account can now hold one or more **passkeys**: the fingerprint,
  face or PIN reader on your own device signs you in, and the private half never
  leaves that device. It is a *separate* credential — your password and your
  two-factor code keep working exactly as before, and revoking a passkey removes
  neither. Account → **Passkeys** lists them with a name you can change, when
  each was added and last used, and whether the authenticator confirmed you with
  a PIN or biometric; **Add a passkey** runs the browser's own sheet; revoking
  one asks first. The login screen grows a **"Sign in with a passkey"** button
  when the browser can do it — and a cancelled prompt says it was cancelled
  rather than telling you your password was wrong. Everything the WebAuthn spec
  makes security-relevant is verified **server-side, in this tree**: the
  ceremony type, the one-shot challenge (single use, five minutes), the origin
  (exact equality — `https://vault.home.evil.example` is refused for
  `https://vault.home`), the relying-party id hash, the user-presence and
  user-verification flags, the COSE key (ES256 or EdDSA only), the signature over
  `authenticatorData ‖ SHA-256(clientDataJSON)`, and the signature counter (a
  regression is refused as a possible clone). Only the **public** key is stored.
  Three things the feature says plainly instead of implying: attestation is
  **not** verified (the format is recorded as evidence, nothing more); a passkey
  is bound to the hostname the appliance is configured with, which the card shows
  because a passkey created for another address will simply not work; and a
  locked account cannot be entered with a passkey either, or a passkey would be a
  way around an administrator's lock.

- **⌘K now finds your files, not just your apps (2026-10-31, QUICK-WIN
  QW-12).** The desktop search overlay answered "which app or setting is called
  this" and nothing else; the Explorer could only search the folder you were
  already standing in. So "where is that contract from last spring?" still meant
  opening the Explorer and guessing which share it was in. One query now fans
  out over **every root the signed-in user may read** — their home directory,
  the shared folders, the attached pools — merged, deduplicated and ranked
  (exact name first, then prefix, then shorter names, then directories).
  Each result shows the folder that holds it, so two files called `photo.jpg`
  in different shares are finally distinguishable, and picking one opens the
  Explorer on that folder **with the file already selected**. Three rules keep
  it honest: it is the Explorer's own root list and ACL filter (one definition,
  two callers, so the search can never reach a folder the sidebar would not
  show — admins included); it is bounded end to end (two characters minimum,
  sixteen roots, 1.5 s per root, 4 s total, a capped list); and it **says when
  it is incomplete** — a location that did not answer, or an answer that was
  cut, is stated under the results instead of being disguised as "no such
  file". Name search only: no content index is built.

- **The box can tell you what to fix, and in what order (2026-10-31, QUICK-WIN
  QW-10 / DSM-18).** Health, Firewall, Antivirus, Ransomware and TLS each
  answered one question about one subsystem; nothing answered *"is this box
  safely configured, and what should I do first?"*. The Health screen now
  carries a **Security Advisor** card that does, and only that: nineteen
  checks over state the other screens already compute — an administrator
  exists and none is named `admin`/`administrator`/`root`, every administrator
  uses 2FA, no administrator still holds an administrator-chosen password,
  HTTPS is on with a certificate that is trusted and not about to lapse with
  automatic renewal, the firewall is on **and still lets SSH and the web
  interface through**, SMB refuses guests, requires signing and rejects SMB1,
  no share folder is world-writable, share size limits, snapshots scheduled,
  a configuration backup on disk, failed sign-ins blocked, and HomeVault up to
  date. Each finding says what is true, on what evidence, what fixes it, and
  links to the screen that owns the setting; the score is the weighted
  percentage of passing checks, floored so "99.6" can never read as a perfect
  hundred. Three rules hold the whole thing together: **a signal that cannot be
  read is never a green tick** (it becomes an `info` finding saying so), **the
  score is arithmetic rather than opinion**, and **nothing is cached**, so a
  fixed finding disappears on the very next run without a restart. The advisor
  only reads — there is no write route, every mutation stays behind the card
  that owns the setting — and the update check reuses the cached answer, so
  opening Health never contacts a third party. Administrator-only, because the
  report names accounts, the certificate, the firewall and the shares.

- **The box can be rebuilt on new hardware (2026-10-31, QUICK-WIN QW-11).** The
  settings export carried preferences and nothing else; Settings → System →
  *Full configuration backup* now produces **one archive** with a consistent
  `VACUUM INTO` snapshot of the database (accounts, groups, shares, ACLs,
  quotas, every settings row), the exported settings, and the generated service
  configurations (`smb.conf`, `/etc/exports`, netplan, the SFTP `sshd`, the ufw
  rules) — so a replacement box gets its accounts and its shares back, not just
  its colour scheme. It is optionally encrypted with a passphrase
  (AES-256-GCM under an Argon2id key), and **by default it holds no working
  credential**: sessions, trusted devices and API tokens are removed, private
  keys are left out, and the config allow-list cannot name a secret file.
  Including credentials requires a passphrase of 12+ characters, enforced where
  the bytes are built rather than only in the handler. Every entry carries a
  SHA-256, so a truncated or edited archive is refused *by name* instead of
  restoring half a configuration; a file that was missing at export time is
  recorded with its reason rather than silently dropped; and an archive from a
  newer HomeVault is refused with both versions named. **Restoring is a staged
  swap, never a hot swap**: the archive is validated, staged, and applied at the
  next start — with the previous database kept as `homevault.db.pre-restore` —
  because replacing a live SQLite file underneath running requests is the one
  thing this feature must not pretend to do.

- **One folder can no longer fill the pool (2026-10-31, QUICK-WIN QW-07).** A
  share can carry a **size quota**: Shares → the *Size quota* action sets a
  ceiling in GB, and the card shows `2.5 GB of 10 GB` with a progress bar that
  turns amber at 80 % and red at 95 %. The enforcement belongs to the filesystem
  that holds the share — ZFS `refquota`, a Btrfs subvolume's qgroup, or an XFS
  project quota — so a client writing over SMB or NFS is bound too, not just a
  browser upload. The Explorer additionally refuses an upload or a copy that
  would cross the ceiling (`507` naming used, quota and incoming bytes), because
  a refusal *before* the bytes land beats a filesystem error halfway through.
  Usage is always read back from the filesystem (`zfs list`, `btrfs qgroup
  show`, `xfs_quota`), never estimated. Two honest limits instead of silent
  no-ops: a share on a filesystem this build cannot cap (ext4, tmpfs, network
  mounts) says so and hides the field, and a share that is a plain folder inside
  a Btrfs subvolume is refused with the reason that a qgroup limit would cap
  every share in that subvolume. The filesystem is changed **before** the
  database records the intent, so the card can never claim a ceiling that is not
  in force.

- **"Who is connected, and can I get them off?" (2026-10-31, QUICK-WIN QW-06).**
  Settings → Security → *Connected clients* lists the sessions the file
  protocols hold — SMB (`smbstatus`), NFS (`showmount`) and the SSH/SFTP/FTP
  streams (`ss`) — with the account, the client address and host name, the share
  or export they hold, how many files are open, and the process evidence. It is
  the answer to "why is the disk busy" that the browser-only *Active sessions*
  could never give. One session can be disconnected: SMB closes the **share**
  (the session survives and can reconnect), SFTP/FTP get a `TERM` to that
  connection's process — and an NFS export is honestly marked as not
  disconnectable, because the kernel holds it. The request names a client, never
  a pid: the server re-collects and takes the pid from the fresh observation, so
  a stale or forged id cannot signal an unrelated process. Every probe is bounded
  and runs off the request thread, and **one failing source never fails the
  list** — a box without Samba reports SMB as unavailable with its own reason and
  still shows its NFS mounts. No absolute path is ever exposed: the card answers
  "who", not "what are they reading".

- **A block on a scanned address now survives a restart — and can reach the
  firewall (2026-10-31, QUICK-WIN QW-05).** Settings → Security → *Failed
  sign-in blocking*: after N wrong passwords from one address inside a window,
  that address is refused for a configurable time. The block is a database row
  (`ip_bans`), not in-memory state, so it is still in force after a reboot — the
  in-memory limiter was the previous answer and it forgot everything on restart.
  It is checked **before** the password is verified, so a blocked address cannot
  keep counting against an account; that, plus the ban being keyed by *address*
  and never by account, is what stops one attacker from locking a legitimate
  user out from somewhere else. Optionally the ban is also installed as
  `ufw deny from <ip>`, so it reaches file sharing and SSH and not just the web
  sign-in; the row records whether that rule was *really* installed, and a
  one-click **Pardon** removes exactly what it added. Three things it will never
  do: block loopback, block an unparsable address, or block the address the
  request itself came from (an admin mistyping their own password must not lock
  themselves out of the box they are configuring). Expiry is self-healing — the
  sweep runs on every enforcement check, so an old ban lifts itself without a
  scheduler or a restart. One more row caps unauthenticated requests per minute
  per address, so a flood is refused instead of reaching the database; it
  deliberately ignores signed-in traffic, because a family behind one router
  shares that budget.

- **The box can wake a sleeping machine again — and scheduled tasks can wait
  for it (2026-10-31, QUICK-WIN QW-09).** Settings → System → **Wake-on-LAN**
  keeps a list of machines by network-card address; "Wake" sends the standard
  102-byte magic packet to ports 9 and 7 over the interface that actually owns
  the target's segment (a frame is a *link-layer* broadcast, so the directed
  broadcast of the right interface is what reaches a powered-off NIC), with an
  optional explicit destination for a relay on another VLAN. The answer is
  **"sent — this only confirms it left"**, never "woken": a UDP datagram is
  unacknowledged and the target is off by definition, so claiming success would
  be a lie in exactly the case the feature exists for. Any scheduled task may
  now carry a `wake` pre-step (`host_id`, `delay_seconds` 0–600), validated when
  the task is saved — a `delay` typo or a missing host is a 400 rather than a
  03:00 run that wakes nothing — and a host that vanished or cannot be reached
  **fails the run** instead of letting a backup hit a sleeping machine. MACs are
  accepted in the spellings an admin pastes (`AA:BB:…`, `aa-bb-…`,
  `aabb.ccdd.eeff`, bare hex) and refused when they are not one machine's
  (`FF:FF:FF:FF:FF:FF` is every NIC on the segment). The interface inventory is
  read from `/sys/class/net` — opening the card forks nothing.

- **The audit trail can finally leave the box (2026-10-31, QUICK-WIN QW-08).**
  Settings → Monitoring → **Remote syslog** forwards HomeVault's own event
  stream to a collector over UDP, TCP or TLS, framed as RFC 5424 or the older
  RFC 3164, with a configurable facility. Two properties make it safe to switch
  on: a log call **never waits on the collector** (it pushes a string into a
  1024-entry queue and returns, so a dead collector can only fill a queue, never
  slow a sign-in), and the feature is **inert while disabled** — the hot path is
  a cached flag, not a JSON read. When the queue overflows the *oldest* entries
  are dropped and counted, and the card shows waiting / sent / dropped next to a
  status pill (`idle`, `forwarding`, `degraded`) so losses are visible instead
  of silent. TLS trusts **only** the CA the admin pastes: the public root list
  is deliberately not loaded, which is what makes the pinning meaningful for a
  self-signed home collector. "Send a test message" reports the collector's own
  reason, and says "sent — check the collector", because a UDP datagram can only
  ever claim the packet left. The host's own `/var/log` sources are deliberately
  not re-shipped (the OS already collects them).

- **Share links can finally be "send this to my neighbour, once" (2026-10-31,
  ROADMAP-DSM QW-04).** A link used to have one knob: time. It now also takes a
  **password** (Argon2id, the same primitive as user passwords) and a
  **download cap**, which is what makes a one-time link (`max_downloads = 1`)
  expressible at all. A download is counted only when the last byte was
  written, so an interrupted, refused or abandoned transfer does not spend it;
  the reserved slot is released on a read error and on a client disconnect,
  which a capped link cannot survive leaking. The owner sees `2/2` and a
  "Download limit reached" badge in the modal instead of a link that silently
  stops working.

  The password never enters a URL. The recipient's page (`/share/<token>`)
  `HEAD`s the link, asks for a password only when the link has one, posts it
  once to `POST /share/{token}/unlock`, and receives a short-lived (15 min),
  path-scoped, `HttpOnly` unlock cookie; the download is then an ordinary
  navigation, so `Range`/resume work and a large file is not buffered into a
  JavaScript blob first. API clients can still send `x-share-password`. Guessing
  is bounded to 8 wrong passwords per link per 5 minutes (checked *before*
  Argon2 runs), while an empty password — the probe the page makes to decide
  whether to show the field — costs nothing. A `401` says `password` or
  `session` in `x-hv-share-auth`, so a signed-in-only link explains itself
  instead of looking broken.

  Five defects of the in-flight implementation were fixed on the way, each one
  user-visible: the `HEAD` probe reported a spent link as downloadable and only
  failed at the actual download; a failure between reserving a slot and opening
  the file leaked the slot forever; a client that disconnected mid-stream leaked
  it too; **a completed download was recorded as interrupted** because hyper
  stops polling a body once its `Content-Length` is written (so a one-time link
  was never spent — a `Drop`-time `sent == expected` check is the fix, and a
  new test reproduces hyper's framing); and **the service worker cached
  `/share/` responses cache-first**, so a spent or revoked link kept working
  from the browser cache (it now skips `/share/`, and the server sends
  `Cache-Control: no-store`). A browser navigation that cannot be answered with
  the file (`Accept: text/html`) now gets the public page instead of JSON, and
  `signed_in` links honour the `hv_session` login cookie because a navigation
  cannot send a header. `GET`/`HEAD` list responses still never carry
  `password_hash` or `active_downloads`. Tests: 11 unit
  (`handlers::share_links`, up from 2) and 8 new integration tests
  (`share_links_integration.rs` **13 → 21**); E2E §16.4/16.5 —
  **16.1–16.5 all green live on Firefox and Chromium** (2026-10-31, scratch
  server `:8099`). Docs: `docs/guides/explorer.md`, `docs/API.md`,
  `SCREENS.md`.

- **A Cloud Sync job can finally run on its own (2026-10-24, ROADMAP 16.4).**
  Until now `POST /rclone/jobs/{id}/run` was the only way a saved sync job ever
  ran: the row held the definition and nothing else, so a nightly sync could not
  be expressed and the screen's "last run / status" columns were the outcome of
  the click that had just happened. A job now carries a cadence — an
  **interval** (1 minute to 366 days) or a **cron** expression evaluated in the
  appliance timezone, like a Scheduled Task — and `next_run_at` is computed by
  the server on create, on edit and after every run. A scheduler spawned at
  startup picks up whatever is due and runs it **outside any HTTP request**, so
  a six-hour sync no longer holds a connection open. Everything new is optional
  with "no schedule" as the default: an existing appliance behaves exactly as
  before until an admin opts in. A cadence that cannot be computed (an interval
  with no value or out of range, an unparseable cron) is refused when the job is
  saved, naming the field, instead of producing a job the scheduler would retry
  on every tick.

- **The run state is the server's, not the screen's (2026-10-24, ROADMAP
  16.4).** `last_run_at`, `last_status` and `last_message` are written by
  whichever path ran the job — the "Run now" button or the scheduler — so what
  the screen shows is a fact. "Run now" answers with the updated job, and a
  failed run now shows its reason on the row instead of leaving the badge on
  whatever the previous attempt left. Two entry points can start a run, so a
  job cannot run twice at once (a click during a scheduled run is refused).

### Security

- **Measured process isolation (2026-11-08, P2-7, partial).** The systemd
  unit is now rendered by `render_service_unit` and ships a conservative
  hardening block: `ProtectSystem=true`, `ReadOnlyPaths=-/opt/homevault`,
  `PrivateTmp=true`, `RestrictSUIDSGID=true`, `LockPersonality=true`,
  `RestrictRealtime=true`, `ProtectKernelLogs=true`, plus
  `NoNewPrivileges=true` in the root deployment. Directives that would break
  a shipped feature (`ProtectKernelTunables` for the WireGuard `sysctl`,
  `ProtectKernelModules` for iSCSI `modprobe`, `ProtectHostname`,
  `ProtectClock`, `PrivateDevices`, `ProtectHome`, `CapabilityBoundingSet`)
  are deliberately absent and documented. `HV_APPARMOR=1` installs an
  opt-in, **complain-by-default** profile (enforce with
  `HV_APPARMOR_ENFORCE=1` after field validation); if the LSM is absent the
  installer warns instead of claiming confinement. The install log now
  prints `systemd-analyze security homevault`. Tests:
  `installer/tests/unit-hardening.test.sh` (written, not run).

### Fixed

- **The unit gate is green again — and it was hiding real bugs (2026-10-27).**
  `cargo test --lib` had been failing on `main` since 2026-10-25 (`1698 passed;
  22 failed`), which turned every required check red and made the branch
  unmergeable. Sixteen of the 22 were assertions that had drifted away from the
  code they describe; behind the other six were product bugs:

  - `DiskService::not_mounted_error` matched `"not in /etc/mtab"`, which
    util-linux's `not found in /etc/mtab` does **not** contain — a teardown
    aborted on a device that was already unmounted.
  - `dhclient` (Setup wizard / Network: DHCP lease renew and release) had no
    install source at all: `isc-dhcp-client` was in neither the inventory nor
    `install.sh`, so the action failed on a Debian netinst or Raspberry Pi OS
    (dhcpcd) with nothing to install. `busctl` (mDNS hostname) was likewise
    absent from `BASE_BINARIES`. The audit that exists to catch exactly this was
    permanently red because it scanned **test** fixtures too — including its own
    needle — so it could never report a real gap.
  - `settings_persistence::export()` re-emitted the legacy `theme`/`density`
    keys that ROADMAP 26.2 promised were no longer written.
  - `api_key_header` let a non-ASCII Syncthing key through
    (`HeaderValue::from_str` accepts obs-text), so the value went out
    byte-for-byte and matched no daemon, while the error message already claimed
    such keys were refused.
  - `validate_layout_blob` started its depth count at 1, so a valid four-level
    layout was refused (and `MAX_LAYOUT_DEPTH` meant three).
  - `normalize_language` kept the first subtag and so accepted
    `en-US-extra-extra` as English.
  - `parse_timezone` was case-sensitive although `docs/guides/scheduled-tasks.md`
    documents `europe/paris → Europe/Paris`.
  - `trim()` ran before the control-character checks in `validate_mount_point`,
    the Syncthing folder ID and the UPS name/variable, so a trailing newline was
    silently removed instead of refused; a UPS name starting with `-` was
    accepted outright.
  - `PathGuard::validate` and `resolve_unconfined` accepted a `.`/`..` leaf —
    both `file_name()` and `components()` normalise a trailing `.` away — and
    returned an in-root **directory** as if it were the file to create.
  - `run_timed_async_with_stdin` discarded the child's stdout (`Stdio::null()`)
    while its synchronous twin keeps it, so `sudo tee` callers lost the tool's
    own message.
  - `Database::wal_checkpoint` discarded the `(busy, log, checkpointed)` row
    `PRAGMA wal_checkpoint(TRUNCATE)` returns, so a busy checkpoint was a
    silent no-op: the `-wal` stayed at hundreds of kilobytes and the backup
    copy of the `.db` file could miss its newest pages. It now pins one
    connection, sets `busy_timeout` on it (a `PRAGMA` is per connection, and
    `configure_sqlite` only reached the connections it happened to use),
    retries while busy and fails loudly if it cannot fold the log.

  Three source-scanning guards (`share_links`, `vm`, `dependencies`) matched the
  needle they were asserting on and could never pass; they now build it with
  `concat!`. Verification: `cargo test --lib` **1720/1720**,
  `cargo test --locked --tests` compiles every target; the remaining
  integration-test drift (`integration.yml`) is tracked separately.

- **The integration suite is green again, and two more real bugs fell out
  (2026-10-27).** `cargo test --locked --tests` now passes end to end (62 test
  binaries). Beyond the `401`-vs-`400` drift (~16 assertions still expected the
  pre-ROADMAP-X.8 status), three product bugs surfaced:

  - **"Mark all notifications as read" never worked.**
    `NotificationRepository::mark_all_read` ran `INSERT … SELECT … ON CONFLICT`,
    which SQLite parses as a join constraint (`near "DO": syntax error`) unless
    the SELECT ends with `WHERE true`. Every call answered `400`; nothing could
    mark the feed read in one action.
  - **A Docker build context outside the allowed roots** was only refused
    *after* probing the daemon, so the answer was "Docker is not available" on a
    host without Docker — and the refusal depended on an unrelated fact. The
    path check now runs first.
  - **The refusal reason for the envelope modules** (Syncthing, Downloader,
    VPN) was re-derived from the HTTP status, which cannot tell a missing token
    from an expired one (both `401`), so a caller who never sent a token was
    told "Invalid or expired session". `admin_guard` now exposes the decision
    itself (`Denial` + `check_auth` / `check_admin`) and those modules relay it.

  Test-harness fixes in the same change: five tests replaced `PATH` with the
  shim directory instead of prepending it, so the shim's own `sleep`/`cat`/`rm`
  vanished and the probe they were timing never ran; the VM test router lacked
  the `/api/vm/isos/download` route the real router registers (405); three
  suites read `body["error"]` where the envelope carries `message`; the 16 KiB
  layout boundary blob was one over-long string (refused by the per-value cap)
  instead of many short values; and the cache-mover test polled "not running"
  before the spawned run had claimed its slot, racing the second trigger.

- **A VM's screen console no longer stays alive on a half-open connection
  (2026-10-24, ROADMAP 32.1).** The noVNC relay was a raw two-way byte pipe with
  no ping and no timeout, so closing a laptop lid, dropping Wi-Fi or suspending
  the tab left the relay task *and* the qemu VNC session alive until the
  kernel's TCP retransmission gave up — many minutes on a quiet LAN, with the
  display held for nothing. It now pings the browser every 30 s and drops both
  sides after 90 s of silence, driven by a clock both pumps and the heartbeat
  share. The browser answers a `Ping` on its own, so a frozen page still counts;
  VNC traffic from the server counts too, so a genuinely live but untouched
  session is not killed.

- **An ISO download is refused on its declared size before a byte is written
  (2026-10-24, ROADMAP 32.2).** The downloader streamed the whole body to disk
  and only then checked the 10 MB floor, so a link returning a multi-GB wrong
  file was fully written before it was rejected — and a 2 KB HTML error page
  still cost a disk write to discover. The declared `Content-Length` is now
  classified up-front: below `MIN_ISO_BYTES` (10 MB) or above the new
  `MAX_ISO_BYTES` (32 GiB) fails the job with nothing created. The same budget
  caps the running write, so a chunked response (no `Content-Length`) and a
  server that lied about its length are stopped mid-stream and the partial file
  removed.

- **A chunked upload no longer forks `id -nG` ~128 times for one file
  (2026-10-24, ROADMAP 33.1).** Every quota check resolved the caller's groups
  with a fresh `id -nG`, and the resumable-upload path re-checks per chunk — a
  1 GB upload in 8 MB chunks forked 128 times. Membership now comes from a
  process-wide cache keyed on the username with a 5 s TTL; the Groups screen
  invalidates the entry on a membership change, so adding someone takes effect
  immediately rather than "within a few seconds", and the list an admin reads
  bypasses the cache entirely.

### Security

- **The File Explorer resolves the last path component instead of echoing the
  caller's string (2026-10-24, ROADMAP 10.5).** Two holes with one root cause.
  `PathGuard::validate` returned the **raw input** when the leaf did not exist
  yet ("a new file"), so `..` survived validation and the leaf was whatever was
  typed; and `validate_path_access`'s admin branch returned
  `PathBuf::from(path)` for *every* admin path, so on that branch nothing was
  canonicalised at all — a symlink planted between the check and the `open` was
  followed with no second look. Both now canonicalise the parent and re-read the
  leaf with `symlink_metadata`: a link that resolves inside the roots becomes
  its target, one that resolves outside is refused, a regular leaf that appeared
  meanwhile is resolved and re-checked, and an absent leaf comes back under its
  resolved parent (no `..`, no dangling textual path). A `.`/`..` leaf is
  refused outright. Admins are still **not** confined to the allowed roots — the
  fix normalises, it does not restrict.

- **A upload chunk is streamed, not buffered in RAM (2026-10-24, ROADMAP
  10.6).** `/explorer/upload/chunk` took an `axum::body::Bytes`, so the whole
  chunk was materialised in memory *before* the handler ran — with the
  router-wide 10 GiB limit as its only bound, one request could allocate the
  machine's memory. It now takes a stream and writes it out frame by frame
  (peak memory: one network buffer), capped at the new
  `MAX_UPLOAD_CHUNK_BYTES` (32 MiB) and at the session's remaining size. Over
  either cap the answer is `413` with the offset to resume at, the temp file is
  truncated back, and the session offset does not move — so the resumable
  protocol's "the offset only advances on a full chunk" invariant holds.

- **A closed share-stream semaphore is a `503`, not a panic (2026-10-24, ROADMAP
  10.7).** The public download path called
  `SHARE_SEMAPHORE.acquire_owned().await.unwrap()`: if the semaphore were ever
  closed that is a bare 500 with nothing in the request log, and a task abort in
  a debug build. It cannot happen with the current static, which is exactly why
  an `unwrap` on a resource whose lifecycle lives elsewhere is a latent panic.
  It now logs and answers `503`; the 4-stream ceiling is unchanged.

### Added

- **The interface now follows the reader's text size (2026-10-25, ROADMAP
  34.1).** Every type step in the product — the twelve `--hv-type-*` roles from
  11px captions to the 62px gauge — was a literal `px`, and `html` was
  `font-size: 16px`. The result was that raising the browser's configured font
  size changed nothing: measured on the login screen, an 11px label and a 13px
  body rendered identically at a 16px **and** a 32px root. Browser zoom still
  worked, so nothing looked broken to anyone who tested with `Ctrl`+`+` — but
  the setting a low-vision user actually changes was ignored, on the first
  screen every user sees, which is exactly what Apple's HIG (*Layout →
  Adaptability*) and the spirit of WCAG 1.4.4 ask an interface not to do. The
  ladder is now expressed in `rem` against `html { font-size: clamp(100%, 100%,
  20px) }`: pixel-identical at 100%, a real range up to a documented 1.25×
  ceiling (past it, dense desktop chrome starts clipping at the widths a NAS is
  administered from — macOS and Windows cap their own text-size setting the
  same way). Three defects found by the same review went with it: a login field
  box declared `h-11` was a **ceiling** that clipped the 16.25px text a 1.25×
  reader gets (now `min-h-11`, so the 44px touch minimum still holds), the
  "Forgot password" link had **no hit area** at all (19.9px tall, `padding: 0`
  — the only interactive element on that screen without a 44px target, now
  `h-11`), and the gateway card's width alternated between two values for one
  viewport (now deterministic).

### Changed

- **The design-system guide stops advertising components that do not exist
  (2026-10-25).** The §4 layer table listed **30** generated shadcn-svelte
  components; `frontend/src/lib/components/ui/` contains **11** (`alert`,
  `badge`, `button`, `checkbox`, `dialog`, `input`, `radio-group`, `select`,
  `sonner`, `spinner`, `switch`), and **none of the 19 extra ones is imported
  anywhere**. The doc described an intention, not the code — the failure mode
  ROADMAP X.4 warns about, with a concrete cost: a developer reads "use
  `Progress`", greps for it, finds nothing, and hand-rolls a bar in the screen,
  which the same section forbids. The row now names the real 11 and a new
  section maps each would-be component to what the tree already uses instead
  (`role="progressbar"` bars, the `.hv-skeleton` utilities, the native
  `<title>`/`<textarea>`/`<details>`, `DataTable`'s server-side pagination …).
  Nothing was generated: 19 empty files would be dead weight, and every pattern
  they would cover is already solved. The rule for the future is written down —
  add a component the day a screen needs it, and update the row in the same
  change. A Rust test now enforces the list **in both directions**
  (`dependencies.rs`, walking `ui/` against the guide), so the doc cannot drift
  again.

- **The roadmap's open-work list is empty (2026-10-24, ROADMAP 31.1).** With
  the Docker screen moved onto `api.ts`, the last item left the list — every
  P0, P1, P2 and cross-cutting finding is closed, and `ROADMAP.md` now says so
  instead of keeping a stale "1 open finding" index. The file also states what
  an empty list means: it lists defects and debt *found by auditing the current
  code*, so "empty" is not a claim of feature-completeness. New entries arrive
  from a fresh audit pass, a live-hardware pass, or a feature request written
  up as a design note first.

- **One HTTP client, everywhere: the Docker app stops hand-rolling its own
  (2026-10-24, ROADMAP 31.1).** `Docker.svelte` was the last screen talking to
  the server with a private helper — a `fetch('/api' + path)` that read the
  session and CSRF tokens out of `localStorage` itself — while `api.ts` exposed
  six Docker methods for the desktop monitor. The two only stayed identical by
  hand, and the private path silently skipped everything else the shared
  client does: a stale cached bundle got no `X-HomeVault-Contract` header and
  therefore no "reload the app" prompt (the contract middleware only gates a
  header that is present), and nothing on the screen took part in the GET
  cache or its invalidation rules. The whole surface — containers, images,
  networks, volumes, `docker system df` and prune, registries, `daemon.json`,
  Compose, community stores, the app catalog, the log ticket and the port
  check — now goes through `api.ts` (59 wrapper methods, 61 rewritten call
  sites), and the wire shapes moved out of the component into
  `frontend/src/lib/types/docker.ts`. Two behaviour changes came with it, both
  deliberate: any Docker mutation evicts the whole `/docker` cache family (a
  container action changes the stats list, the no-stats list *and* the
  catalog's installed/running flags; an image pull changes the disk-usage
  table), and `get()` reads `ttl ?? default` instead of `ttl || default` so a
  deliberate `ttl: 0` is no longer silently turned into the 5 s default. Every
  new wrapper is uncached, so nothing the screen reads is served from a TTL
  and the request count per interaction is unchanged. Covered by a new
  `docker.spec.ts` §17 that runs without a docker daemon: all three headers
  present on every Docker request, no raw `/api` fetch in any served chunk,
  and a container action that must not be answered from the cache.

### Fixed

- **No host command anywhere runs without a deadline (2026-10-24, ROADMAP
  X.9).** X.2 closed this "for the whole tree" — but only the modules that
  received a per-module source guard were ever re-checked, so **98 sites across
  26 files** still invoked a subprocess with no ceiling: `getfacl` on a dead NFS
  mount, `getent`/`id` behind an unreachable NSS backend, `sudo` waiting on a
  password prompt, `mount --bind` on an unreachable export, `chpasswd`, `git
  clone`, `docker compose pull`, `nvidia-smi` on a wedged driver, `visudo -c`,
  `chown -R` on an unmounted home. Each one could pin the thread serving the
  request — the Explorer's ACL view, the Users/Groups screens, `GET /shares`,
  the login path, the setup wizard — with nothing to time it out. All of them
  now go through `utils::process` or a module-local wrapper built on it, with a
  ceiling chosen for what the command actually does (10–15 s for a probe, 30–60 s
  for a write or a service action, minutes for a network fetch). The four
  children whose *lifetime is the work* (`clamscan`, `ddrescue`, `photorec`, the
  TestDisk scan) stay unbounded on purpose — they are cancelled through their
  process group and now carry `kill_on_drop` — and those, plus the two command
  *builders*, are named exemptions in the guard.
- **The rule is now enforced tree-wide, not by hand.** A new source guard
  (`utils/process::tests::every_subprocess_of_the_tree_is_bounded`) walks `src/`,
  finds every `Command::new` and fails on one whose invocation is not wrapped.
  That is the guard X.2 was missing: per-module guards cannot notice a module
  nobody listed.

### Added

- **A switch for local-network discovery — and it really removes the
  advertisement (2026-10-24, ROADMAP 29.1).** `crate::mdns::unpublish()` had
  **no caller at all**: the Avahi service file and the `host-name` override
  stayed on disk whatever the appliance was asked to do, so the only way to stop
  broadcasting `homevault.local` was to disable Avahi by hand. Settings →
  General now carries **"Discoverable on the local network"**
  (`GeneralSettings.mdns_enabled`, shipped **on**), and the save reaches the
  host: turning it off deletes `/etc/avahi/services/homevault.service`, strips
  the `host-name=` line from `avahi-daemon.conf` (`remove_host_name`, the exact
  inverse of `ensure_host_name`) and renames the running daemon back; turning it
  back on republishes **immediately** instead of waiting up to 120 s for the
  reconciler. The reconciler now skips its tick while disabled (it used to
  recreate the file two minutes later), reset-to-defaults republishes, and a
  host that refuses the change answers `200` with `mdns_notice` shown in the
  card — a save that could not reach the host must not look saved.
  `GET /mdns/status` and `GET /settings/general` report the **live** flag.

### Fixed

- **A host-name change no longer drops every other mDNS advertisement on the
  host (2026-10-24, ROADMAP 29.2).** `avahi-daemon` reads `host-name=` at
  startup only (`--reload` re-reads `/etc/avahi/services/*`, *not*
  `avahi-daemon.conf` — see `avahi-daemon(8)`), so a rename restarted the
  daemon and briefly removed **everything** it published for that host: the
  admin's printers, other appliances' shares. The rename now goes through
  Avahi's D-Bus `SetHostName` (`sudo busctl call`), **verified by reading
  `HostName` back** so a call that silently did nothing still falls back to a
  restart (logged at `warn`). The config file is still written for the next
  boot.

- **A background job that fails keeps its row instead of vanishing (2026-10-24,
  ROADMAP 28.2).** The activity feed filtered on `pending | running`, so a job
  that failed one second ago disappeared on the very next poll — the row the
  user was watching simply vanished, and the only trace left was a notification
  in the bell. A `failed` or `cancelled` job now stays for 60 s after its last
  event and carries a `state` slug, and the desktop renders it as a failure: no
  progress bar (there is no progress to report), the localised word instead of a
  percentage, and **no dock badge** — the badge counts work in progress, so the
  dock button now keys off "is there a row to report" while the badge counts
  active rows only.

- **A failing backend is no longer polled twice a second (2026-10-24, ROADMAP
  28.1).** The activity store used `setInterval(poll, 2000)` unconditionally, so
  an appliance answering `500` was hit 30 times a minute per open desktop — a
  self-inflicted load spike exactly when the box is already unwell. The loop
  backs off exponentially (2 s → 4 → 8 → 16 → 32 → 60 s cap), tolerates one
  transient miss so the panel does not flicker, and resets the delay when the
  tab becomes visible again.

- **`mdns.rs` runs no subprocess without a deadline (2026-10-24, ROADMAP X.2 /
  X.9).** Five bare `Command` invocations (`sudo tee`, `sudo rm`, `systemctl`,
  `which`, `avahi-daemon --reload`) could pin the reconciler thread forever on a
  `sudo` prompt waiting for a terminal. All of them now go through
  `utils::process::run_timed` / `run_timed_with_stdin` with three ordered
  ceilings, and a source guard fails the build if a bare invocation comes back.
  The sweep this prompted found **98 more such sites in 26 files**, now tracked
  as ROADMAP X.9.

### Security

- **One auth primitive, one status contract — a non-admin is `403` and a
  missing session is `401`, everywhere (2026-10-24, ROADMAP X.7/X.8).** Two
  defects, both invisible to a test that only ever sends a *valid* token.
  **First**, `handlers::admin_guard` was meant to be the shared session check but
  nine modules carried their own copy: `handlers::auth::require_admin` (a
  duplicate of the shared one) and private `require_admin` helpers in
  `syncthing`, `shares`, `vpn`, `ups`, `permissions`, `snmp`, `user_homes` and
  `downloader`, plus 65 inline `x-session-token` blocks across `security.rs`
  (6), `settings.rs` (20), `explorer.rs` (31), `chat.rs` (7) and `avatars.rs`
  (1), plus 11 dead re-checks in `docker.rs` that ran *after* a successful
  guard. The copies had drifted: `shares`, `permissions`, `snmp` and `settings`
  answered **`400 Bad Request`** to a signed-in non-admin where every other
  module answered `403`, so the same refusal looked like two different problems
  on two screens. **Second**, `require_auth` answered `400` when the
  `x-session-token` header was *absent* and `401` only for an invalid or
  expired one, so a client that branches on the status read "you are not signed
  in" as "your request is malformed". `admin_guard` is now the only module that
  turns the header into an identity decision: the copies delegate to it, and it
  gained `require_auth_caller` (username + own token, for the self-service
  session endpoints), `require_session_token` (the modules that still resolve
  the `Session` row themselves — the Explorer's path guard) and
  `denial_message` (status → wording, for the modules that answer a
  `Json<ApiResponse>` envelope: Syncthing, Downloader, VPN). The contract is
  stated once, in `admin_guard.rs` and `docs/API.md` → *Auth status contract*:
  `401` for an absent or empty header and for an unknown or expired token,
  `403` for a valid non-admin on an `admin` route, and `400` **only** for a
  request the server could not understand. Four source-guard unit tests fail the
  build if a module defines its own `require_admin`/`require_auth` again or if
  one of the eight delegated modules starts parsing the header, and the new
  `tests/auth_guard_integration.rs` (7 tests) pins the behaviour — including
  the pair that gives the contract its meaning: the same route answers `401`
  without a session and `400` for a malformed body from an admin. 39 no-token
  assertions across 24 suites moved to `401`; the SPA needed no change (it
  never branched on `400` vs `401`).

### Added

- **Syncthing — a folder and a device can be added and removed from HomeVault
  (2026-10-24, ROADMAP 18.1).** The screen was a monitor: it could show the
  daemon's topology and pause/resume/rescan, but there was no route and no
  control to add or remove a folder or a device, so a new phone or a new pool
  folder had to be configured on the daemon directly. Four routes now exist —
  `POST`/`DELETE` on `/syncthing/folders[/{id}]` and `/syncthing/devices[/{id}]`
  — behind the same `require_admin` bar as the rest of the module, and every
  field is validated **before** the first daemon call, because a stopped daemon
  would otherwise answer a transport error and hide the fact that the request
  was never valid. The folder ID is confined to `[A-Za-z0-9._-]` and 64 bytes;
  the label is trimmed and capped at 128; **the path must be absolute, free of
  `..`, not `/`, and inside `state.path_guard.allowed_roots()`** — Syncthing
  creates the directory, so an unconfined path would let an admin write anywhere
  the service can; the sync mode is the allowlist
  `sendreceive|sendonly|receiveonly`, with `receiveencrypted` refused; a device
  ID is normalised to eight groups of seven base-32 characters with dashes and
  case optional (the check digit is left to the daemon, which is the authority,
  and its refusal is relayed) and control characters are refused. The config
  read that follows doubles as the duplicate check and as the allowlist of
  devices a folder may be shared with; the appliance's own ID is injected into a
  new folder's `devices` array and pairing or removing it is refused, so the
  host cannot be unpaired from itself. Deletes re-validate the id from the URL
  and percent-encode it before it reaches the daemon's REST path. On the screen,
  each tab gained an *Add* action opening a dialog that keeps the server's
  refusal **inside** the dialog with the typed values intact, a success banner
  naming what happened, and a *Remove* button on the detail pane whose
  confirmation states the consequence (the files already on disk stay). The path
  field is `PathField` rather than the read-only picker because Syncthing
  creates the target directory; "Share with" pre-ticks every paired device;
  "start paused" is omitted (simplicity rule 2). Tests: `syncthing.rs` 18 → **34**
  unit, `syncthing_integration.rs` 8 → **12**, `syncthing.spec.ts` 23 → **34
  runtime / 30 `test(`** (section 7 drives a mocked daemon, so no real daemon
  configuration is ever mutated). **Written but not executed** (per instruction):
  no suite was run. Docs: `docs/guides/syncthing.md` (validation table, *Known
  debt*), `docs/API.md`, `ROADMAP.md` (item 18 closed, 29 → 28 open findings),
  `TESTING.md`, the RAG entry, and the four `src/i18n/*.toml`.

- **DB backup — one snapshot at a time process-wide, a failure that is visible,
  and a restore point that survives retention (2026-10-24, ROADMAP 22).** **Two
  concurrent `VACUUM INTO`s were possible.** Every HTTP handler built its own
  `DbBackupService` while the "one snapshot at a time" guard was a per-instance
  `Arc<tokio::sync::Mutex<()>>`, so a manual click landing during a scheduled
  run executed both — exactly what `docs/guides/backup-restic.md` promised could
  not happen. The lock is now one module static (`RUNNING` in `src/db_backup.rs`)
  shared by every handler, the scheduler loop, the startup snapshot and
  `snapshot_before_migration` (which used to bypass the service entirely); a
  process-wide static rather than an `AppState` field, because the latter would
  have touched the 40+ `AppState` literals under `tests/`. Two snapshots in the
  same second are named `homevault-…_2.db` instead of one overwriting the other,
  and the `_` suffix keeps the reverse-name sort newest-first. **A failed
  snapshot used to be invisible.** `DbBackupStatus.last_error` was hardcoded
  `None`, so an unwritable directory or a full disk looked exactly like "no
  snapshot yet"; the reason of the last failed attempt is now kept process-wide,
  served by `GET /settings/backup/db`, cleared by the next success and by a
  restart, and the Settings card renders it as a `role="alert"` line quoting the
  server's own words. **Retention could eat the pre-upgrade restore point.**
  Pruning kept the newest `retain` files by name with no marker filter, so a
  forced `homevault-<ts>.pre-migration.db` — the only copy of the pre-upgrade
  state — could be deleted like any other snapshot, and it also consumed one of
  the retention slots (with `retain = 2`, the old code kept one real snapshot and
  deleted the other). Pruning now partitions the directory: `retain` bounds the
  regular snapshots only, `.pre-migration` restore points are never removed by
  it and are bounded separately at `PRE_MIGRATION_RETAIN` = 3 newest;
  `DbBackupEntry.is_pre_migration` marks them in the API and the card labels them
  *Before upgrade*. **Also (X.1, this module):** every `std::fs` call left the
  async runtime — `create_dir_all`, the stale `.tmp` unlink, `rename` +
  `metadata`, `read_dir`, the retention `remove_file`, and the
  `data/db_backup.json` load/save/delete — and `list`/`prune`/`status`/`is_due`
  are `async` because they reach the filesystem; handlers use the I/O-free
  `DbBackupService::new` plus `load(db).await`, which reads the schedule on the
  blocking pool. Tests: `db_backup.rs` 5 → **11** unit (a snapshot must wait on
  the lock held by *another* instance; prune keeps and bounds restore points; a
  `_2` duplicate sorts newest), `db_backup_integration.rs` 12 → **15** (a planted
  restore point survives three snapshots at `retain = 2` and is flagged; six
  restore points plus a settings save keep the newest three; a failed snapshot is
  a `500` whose `last_error` names the directory, and the next success clears
  it), `settings.spec.ts` §15 3 → **4** `test(`. **Written but not executed**
  (per instruction): no suite was run. Docs: `docs/guides/backup-restic.md`
  (*Only one snapshot runs at a time*, *Retention never eats the pre-upgrade
  restore point*, *A failure is remembered*, *Nothing blocks the async runtime*),
  `docs/API.md`, `ROADMAP.md` (item 22 closed, 31 → 29 open findings),
  `TESTING.md`, and the four `src/i18n/*.toml` (`db_backup_last_error`,
  `db_backup_pre_migration`).

- **Dashboard / Widgets — per-account tiles, deadlines on every host probe, and
  no fabricated measurement (2026-10-23, ROADMAP 15).** **Widgets belong to an
  account.** `handle_add_widget` used to hardcode `username: None` and neither
  handler checked ownership, so any authenticated user could delete or reorder
  every tile on the appliance; migration `057_widgets_per_user.sql` rebuilds
  `widgets` with `(username, id)` as the identity, `username NOT NULL` and
  `ON DELETE CASCADE`, copying the legacy ownerless rows to every existing
  account (they were one grid shared by everybody — dropping them would have
  made tiles vanish on upgrade). `WidgetRepository` lost every unscoped method:
  `find_by_id`, `find_by_username`, `count_for`, `create`, `delete_for → bool`,
  `delete_all_for`, `reorder_for → u64` all take the caller's username, which
  comes from `require_auth` and never from the body. A tile the caller does not
  own is a `404`, not a `403` — the endpoint must not confirm that another
  account owns that id. `POST /dashboard/widget` now validates every field
  before it reaches SQL (id/type slugs ≤ 64 chars from `A-Za-z0-9-_.:`, title
  trimmed and ≤ 120 chars with no control characters, `size` = `NxN` with each
  side `1..=6`, row/col `1..=64`, spans `1..=6`, 64 tiles per account);
  `GET /dashboard/widget/{id}` answers `404` for an id no tile uses instead of
  `200 {"type":"unknown"}`; a reorder list matching none of the caller's tiles
  is a `404` and the ids it does not own are ignored. The two
  widget-preference handlers stopped hand-rolling `SELECT username FROM
  sessions WHERE token = ?` — the statement with **no `expires_at`
  predicate**, which let an expired session read *and write* preferences — and
  go through `require_auth` (invalid or expired → `401`). `monitor_layout` now
  validates the blob structurally (JSON object, ≤ 16 KiB, depth ≤ 4, ≤ 512
  nodes, keys ≤ 64 chars, strings ≤ 256) while still storing the accepted bytes
  **verbatim**, because a `serde_json::Value` round trip reorders keys and the
  frontend relies on a byte-for-byte read. **No host probe runs untimed.** New
  `src/utils/process.rs` (`run_timed`, `run_timed_async`, `HOST_PROBE_TIMEOUT`
  10 s, `SMART_PROBE_TIMEOUT` 20 s) is the single deadline helper: `utils/proc.rs`
  wraps all 20 of its probe sites, `dashboard.rs` wraps its 15 plus the four
  `smartctl` calls, and the journal's private copy of `run_timed` was deleted in
  favour of a delegation. `/dashboard/health` serves a 30 s snapshot coalesced
  by a mutex, so the 5 s poll no longer re-forks `lsblk` + `df` + 1-3 SMART
  commands per disk; the browser keeps its 5 s interval (E2E 3.1 pins it) but
  skips ticks while `document.hidden` or while a request is in flight, and
  refreshes immediately on `visibilitychange`. **Nothing shown as a measurement
  is a constant any more.** The disk score is SMART 40 / temperature 20 / usage
  40 — the hardcoded `error_score = 10` that no disk ever earned is gone, so a
  failing disk no longer starts at 10 and a perfect one reaches 100; the status
  vocabulary is `healthy | warning | unhealthy` through one
  `health_status_for_score` (the fifth word `critical` that nothing rendered is
  gone); CPU % is a per-caller `/proc/stat` delta instead of `load1 / cores`
  (which can exceed 100 %), shared with the health endpoint and both widget
  routes so the Dashboard and the Metrics app cannot disagree;
  `/dashboard/widget/memory` returns real `/proc/meminfo` numbers;
  `has_server_update` was removed from `UpdateInfoResponse` (constant `false`
  in both branches, no consumer); `PREV_NETWORK_STATS` is per account with a
  first-sample rate of `0` and saturating subtraction; `CpuDetails.usage_percent`
  was added because the app-usage card read a field that did not exist. On the
  screen: the static `UP TO DATE` badge and the literal "last test 2" are gone,
  `healthHint` follows the real status, network rates use the real elapsed time
  and `iface.link_speed_mbps` (it read `iface.speed`, always `undefined`), the
  Docker card is gated on `$isAdmin` (it used to poll an admin-only endpoint
  from every viewer every 5 s) and surfaces a failed action in a `role="alert"`
  band, `monitorStore` gained `savedAt` so a login on a second origin cannot
  revert a fresh layout, and the dead `app-update`/`server-update` routes were
  removed from `widgetData.ts`. Tests: `dashboard.rs` 23 → **40** unit,
  `monitor_layout.rs` 0 → **13**, `proc.rs` 0 → **7**, `utils/process.rs` **5**,
  `dashboard_integration.rs` 10 → **19**, `widgets_integration.rs` 9 → **14**,
  `dashboard.spec.ts` 40 → **45** `test(`, `widgets.spec.ts` 20 → **22**.
  **Written but not executed** (per instruction): no unit, integration or E2E
  suite was run. The tree-wide `400 → 401` for a *missing* `x-session-token` was
  deliberately **not** flipped (it is one line in `admin_guard` plus a ~50
  assertion sweep across ~25 suites that cannot be validated without running
  them) and is recorded as ROADMAP **X.8** with its exact scope.

- **Downloader (aria2) hardening — validated input, a secret on the RPC port,
  and a child that is actually owned (2026-10-23, ROADMAP 17).** **Nothing the
  request says reaches the RPC unvalidated any more.** `uri` goes through a
  scheme allowlist (`http`, `https`, `ftp`, `sftp`, `magnet`), so `file://`
  (which read `/etc/shadow` as the service user), `data:`, `javascript:` and
  `smb:` are refused with a reason; plain links must carry a host, magnet links
  an `xt=`. `dir` changed meaning: it is now a **relative sub-folder of one
  configured root**, `<DATA_DIRECTORY>/downloads` (`paths::downloader_dir()`),
  with absolute paths, `..` and backslashes refused — and
  `resolve_download_dir` canonicalises the deepest existing ancestor, so a
  symlink planted inside the root cannot redirect a download outside it. `out`
  must be a bare file name. All three validators run **before** the daemon is
  touched, so a refused request never spawns aria2, and `dir` is always sent
  explicitly so the daemon cannot fall back to its own working directory. The
  event log redacts `user:pass@` in a link. **The RPC port is no longer open to
  any local process:** each start generates a fresh `rpc-secret` (`hv_` + 128
  bits) written to `<DATA_DIRECTORY>/aria2-rpc.conf` with mode 0600 and passed
  through `--conf-path` — never `--rpc-secret` on the command line, because
  `/proc/<pid>/cmdline` is world-readable — and every call prepends
  `token:<secret>` under a 10 s timeout. The fixed 500 ms sleep became a
  bounded `aria2.getVersion` poll (10 s deadline, 200 ms probe) that kills the
  child if it exits or never answers, so "started" means the port is up. **The
  child is reaped and stopped:** a crashed daemon is detected through
  `try_wait` and reported as `running: false` instead of forever "running",
  `shutdown_signal` kills and waits it on Ctrl+C/systemd stop, and downloads
  moved out of `/tmp/aria2-downloads` (wiped on every reboot) into
  `<DATA_DIRECTORY>/downloads`. `src/handlers/activity.rs` dropped its own
  unauthenticated RPC copy and uses the shared secret-aware helper. **UI:**
  opening the app no longer starts the daemon behind the user's back — a
  stopped daemon shows an explicit "Start the download service" panel; `add`,
  `pause`, `unpause`, `remove` and `start` failures reach a dismissible alert
  instead of five empty `else {}` branches; the free folder picker (which
  offered absolute paths the API now refuses) became a relative "Sub-folder"
  field with the destination root stated underneath; and a task card is a plain
  container whose selectable part is a real `<button>` (it used to be
  `role="button"` wrapping its own action buttons). Tests:
  `src/handlers/downloader.rs` 15 → **32** unit, `tests/downloader_integration.rs`
  8 → **13**, `tests/e2e/downloader/downloader.spec.ts` 40 → **46**.
  **Written but not executed** (per instruction): no unit, integration or E2E
  suite was run.

- **Disk encryption (LUKS2) production-ready pass (2026-10-23).** The three
  ways the encryption tab could destroy data or wedge itself are gone, and the
  two operations that did not exist now do. **Deletion is gated:** `DELETE
  /api/encryption/volumes/{id}` used to run `let _ = unmount(); let _ =
  close(); erase();` — a busy mount made `umount` fail, `cryptsetup erase`
  succeeded anyway, and the key material was gone while the filesystem was
  still mounted and being written to. It now unmounts (a failure answers 400 and
  stops), closes the mapper if `cryptsetup status` says it is open, then
  **re-verifies** the release from the kernel's own view — `/proc/self/mounts`
  (with `\040`-style escapes decoded and the mount point matched as a whole
  field) and `cryptsetup status` — and only then erases; an unreadable
  `/proc/self/mounts` fails closed. Every refusal keeps the metadata row.
  **Unlock rolls back:** `open()` followed by a failed `mount()` closes the
  mapper again (the old code left it open while the UI still said "locked", so
  every retry hit "already mapped"); when even that fails the message names the
  `sudo cryptsetup close` line to run. **Creation is a tracked job:**
  `POST /encryption/volumes` validates synchronously (passphrase policy,
  filesystem allow-list, label, duplicate label, device gate — nothing
  privileged runs on a refusal) and answers `{ job_id }`; the work runs as
  `Custom("encryption_create")` with milestones and one `VOLUME_LOCK`, and a
  failure after `luksFormat` rolls the container back (unmount → close → erase)
  so the device returns to the available pool instead of being stuck "in use"
  with no metadata row. New **passphrase rotation**
  (`POST /encryption/volumes/{id}/passphrase`) sends the three lines
  `luksChangeKey` actually prompts for — the old shape sent one, so it would
  have failed on "Passphrases do not match" — and proves the new passphrase with
  `cryptsetup open --test-passphrase`. New **passphrase policy** for new
  passphrases (12 characters, no wordlist entry, not the label, four distinct
  characters), applied to create and rotation only so existing volumes stay
  unlockable. Also fixed: lock reports the unmount failure instead of closing
  anyway; the progress stream replays a job's last known event on subscribe (a
  job that failed before the modal connected used to be invisible, leaving the
  panel on its bar forever); the fstab write no longer `.unwrap()`s the child's
  stdin; the fstab
  edits are async with unit-tested pure halves; the volume list probes statuses
  concurrently instead of serially; `wipefs -a`'s failure is reported in
  `data.warnings[]` instead of leaving the device unusable with no explanation;
  and every privileged call is bounded (`run_timed` with `kill_on_drop`:
  format 900 s, open/`luksChangeKey` 180 s, close 120 s, erase/`wipefs` 600 s,
  status 15 s, UUID 30 s, mount 120 s, fstab 60 s). UI: the create modal turns
  into a progress panel and a "Change passphrase" modal was added (i18n ×4).
  Tests: `src/encryption.rs` 2 → 24 unit, `handlers/encryption.rs` 0 → 6,
  new `tests/encryption_integration.rs` (15), `storage_integration.rs` updated
  to the new policy, `storage.spec.ts` +1 case and 13.1 rewritten to follow the
  job and prove the rotated passphrase. **Written but not executed** (per
  instruction): `cargo check --all-targets` and `npm run check` are clean, no
  test suite was run.

- **Docker production-ready pass (2026-10-20).** The Docker module gained the
  whole engine surface a NAS console needs: **volumes** (list/create/delete
  with in-use detection), **disk usage** (`docker system df`) and **prune**
  (images / volumes / builders / system — an unknown target is rejected
  *before* the daemon is contacted, never defaulted to a destructive action),
  **container details** (`inspect`), **exec**, **pause/unpause**, **kill**
  (whitelisted signals), **rename**, **in-place limit updates** (memory, CPU,
  restart policy, no recreate), **update to the newest image** (pull, rename
  the live container to `<name>-hv-old-<epoch>`, rebuild from `inspect`, roll
  the rename back on failure, remove the parked container only on success),
  **live log streaming** over a single-use 30 s SSE ticket (merged
  stdout+stderr, tail size 1–5000, timestamps, follow, download), **image
  builds** from a `PathGuard`-validated host folder, **private registries**
  (`--password-stdin`; the list endpoint returns `auths` keys only, never a
  credential), a **`daemon.json` editor** validated as a JSON object before it
  is written and followed by `systemctl restart docker`, and **Compose update**
  (`compose pull` + `up -d`, since `up -d` alone never re-pulls).
  Two systemic defects went with them: no Docker CLI call had a timeout (a
  wedged daemon parked a Tokio worker forever) and every mutation ran on the
  async runtime (one slow `docker` call stalled all concurrent requests) —
  now `run_docker` / `run_command` / `run_command_with_stdin` all funnel into
  `wait_with_timeout` (`DockerError::Timeout`), and every handler mutation
  runs through `spawn_blocking`. Frontend: a 7th **Volumes** tab carrying the
  disk-usage table and the prune buttons, a per-card "more actions" menu, a
  rebuilt live-logs modal and 8 new modals; ~100 new i18n keys ×4 locales.
  Tests: `docker.rs` 9 → 17 unit, new `tests/docker_integration.rs` (10).
  `docker.spec.ts` updated for 7 tabs and extended with a §16 "Production surface"
  (8 tests); re-run against a live daemon: **123 passed / 9 skipped / 0 failed**
  across firefox + chromium, which also caught and fixed three real UI defects
  (both card popover menus clipped by the card's `overflow-hidden`, Escape not
  closing them from the trigger, and a racy container count in test 5.2).

- **Group rename and description (2026-10-20).** `PUT /api/groups/{name}`
  takes `{ name?, description? }`: a rename runs `groupmod -n` and then moves
  every reference that keys on the **display name** — the `hv_groups` mirror
  (name + `full_name`, `gid` preserved), `resource_group_shares` (group-level
  grants), `transfer_quotas` (`subject_type = 'group'`) and
  `share_permissions` (`grantee_type = 'group'`) — so a rename can no longer
  orphan a group's access. The OS rename runs first, so a refusal (name taken,
  no sudo) leaves every reference untouched; the admin group is refused
  outright because it carries the sudoers rule. The description lives in the
  `hv_groups` mirror only (the OS has no such column), is created on demand
  when the mirror row is missing, and `""` clears it. `GET /groups` now
  carries `description`; the Groups detail pane gains an "Identity" card
  (rename + description, read-only for the admin group) and the create form a
  description field. Deleting a group now also drops its mirror row and its
  grants, so a re-created display name inherits neither. Tests:
  `groups_integration.rs` 26 → 34,
  `tests/e2e/groups/group-identity.spec.ts` 3 passed / 3 skipped (the editing
  half needs `groupadd`/`groupmod` over NOPASSWD sudo).

- **Group-level resource access (2026-10-20, migration 051).** Per-member
  grants (`resource_shares`) had to be re-typed for every account, and a member
  added to a group later inherited nothing. `resource_group_shares` keys the
  same `(kind, resource, permission)` triple by HomeVault **group** name
  (no FK — groups live in `/etc/group`), exposed as `GET/POST/DELETE
  /shares/access/groups` (admin-only, event-logged). Effective access is the
  union of direct and group grants, resolved **at read time** from the member's
  live OS membership, strongest level winning — nothing is copied, so leaving
  a group revokes the inherited access on the next request and revoking a group
  row never demotes a member who also holds a direct row. `GET
  /shares/access/mine` gained `inherited`; the Users access card lists the
  group grants under "Inherited from groups" with an editor limited to the
  member's own groups; deleting a group purges its grants. Tests: 3 unit
  (`merge_strongest`), `resource_shares_integration.rs` 10 → 15,
  `tests/e2e/users/group-access.spec.ts` 2 passed / 2 skipped (the
  inheritance half needs `groupadd` over NOPASSWD sudo).

- **Account expiry (2026-10-20, migration 050).** `users.expires_at` (NULL =
  never, the default) gives an account an admin-set horizon: past it,
  `AuthService::authenticate` refuses sign-in before the password is verified
  (`401 Account has expired`, no lockout footprint, PAM fall-through
  included), and `AuthService::revoke_expired_account_sessions` deletes the
  sessions the account still held — once at boot and then from a 10-minute
  loop started with the other schedulers. `PUT /users/{username}/expiry` is
  the only writer (the profile update never moves it) and a past date is
  refused so the mistake surfaces in the UI instead of locking someone out.
  The Users detail pane gains an "Account expiry" card (datetime-local editor,
  local rendering of the stored UTC instant, an "Expired" badge). Tests:
  `users_integration.rs` 66 → 71, `tests/e2e/users/account-expiry.spec.ts`
  4/4 (Firefox, scratch server).

- **Admin per-user session management (2026-10-20).** The Users detail pane's
  Security card now lists a member's active sessions — address, created and
  expiry, token masked to its first 8 characters + `…` — with a "Sign out
  everywhere" button, backed by `GET /users/{username}/sessions` and `DELETE
  /users/{username}/sessions` (admin-only, event-logged bulk revoke). The
  masking is deliberate: a full token read would let an admin replay someone
  else's session, so revocation is bulk-only — what an incident needs, and the
  user simply signs back in afterwards. Tests: `users_integration.rs` 61 → 66,
  `tests/e2e/users/admin-sessions.spec.ts` 2/2 (Firefox, scratch server).

- **Forced first-sign-in password change (2026-10-20, migration 049).** A
  password an admin chose or generated — account creation via `POST /users`, or
  an admin password reset targeting someone else — now sets
  `users.must_change_password`. The owner's next sign-in routes to a forced
  password-change screen on the login shell, change included, nothing else:
  `middleware::password_change_guard` answers `403
  password_change_required` to any other API call the flagged session makes
  (client-side UI is not the boundary). Login, `/auth/me`, the setup response
  and `PUT /users/{u}/password` carry the flag; self changes clear it, an
  admin reset re-arms it, setup and recovery never set it. Members who cannot
  reach the admin-only desktop still get the change screen — the "random
  password handed over out of band" flow now ends with the owner's own
  password. Tests: `middleware/password_change_guard.rs` (5 unit),
  `tests/password_change_integration.rs` (8), `login_integration.rs` 62/62,
  `users_integration.rs` 61/61, `tests/e2e/password-change/` 16/16 (Firefox,
  scratch server). The E2E pass caught two real bugs, both fixed: the guard
  blocked the gate's boot read (`GET /auth/setup-status`), which killed the
  reload-mid-flow path, and cargo does not track `static/dist` for its
  `include_dir!` embed — the binary kept serving the pre-change bundle until
  `handlers/spa.rs` was touched.

- **Network configuration persistence (2026-10-20).** A configuration applied
  from the Network app now survives a reboot: `PUT
  /network/interfaces/{name}/config` persists the applied config to netplan
  (`/etc/netplan/99-homevault-<name>.yaml`, renderer mirrored from the
  existing config, static v4+v6 merged into one `addresses:`/`routes:` pair)
  or, without netplan, to an ifupdown stanza. Without any backend the change
  still applies live and the UI shows a "temporary" warning. The config is
  written, never applied — `netplan apply` is left to boot. **(Closed
  2026-10-21 — see the bullet below.)**
- **Network apply-verify-rollback (2026-10-21).** The persisted interface
  configuration is now **applied and verified in the same request**, closing
  the one-cycle runtime/boot drift. `PUT
  /network/interfaces/{name}/config` captures the pre-change state, applies
  it live, persists it (the write returns the backend plus a snapshot of the
  previous netplan file), runs `netplan apply`, and verifies the wire
  converged within a 10 s polling grace (static address, default route via
  the configured gateway, static IPv6, interface up). On any failure the
  change rolls back on both halves — previous file restored (or ours deleted)
  and the previous live configuration re-asserted, so the operator session
  never loses connectivity — and the API answers `success:false` naming what
  failed; the UI reloads the interfaces after a rollback. Supporting changes:
  scoped sudoers shapes (`netplan apply`, `cat`/`chmod 600`/`rm -f` on
  `/etc/netplan/99-homevault-*.yaml`) and `netplan.io` in the dependency
  inventory + `install.sh` (optional; ifupdown stays write-only and
  boot-applied). Tests: `src/network.rs` 43 → 52 unit (pure verify verdict),
  full `cargo test` green (lib 917, all integration suites).

### Fixed

- **UI action feedback — no mutating handler swallows its failure any more
  (2026-10-24, ROADMAP X.3).** The four areas the item named were already fixed
  (CloudSync, Health, the desktop power menu, NotificationCenter), so the audit
  scanned the whole `frontend/src` tree and found the residual silent branches
  it had missed: **Shares** (service start/stop/restart, all five protocol
  config saves — which ignored the response entirely — SFTP key add/save and
  rebuild, share create/delete/toggle, ACL grant/revoke/sync), **Backup/restic**
  (schedule/retention-policy/repo create, delete and toggle, backup, restore,
  check, forget, prune, unlock, snapshot delete) and one **Storage** site (the
  SMART self-test start). They all go through one new shared helper,
  `frontend/src/lib/utils/notify.ts` — `notifyError(response)` shows the
  server's own `message`/`error` under the translated `common.action_failed`
  fallback, `notifySuccess('common.saved')` confirms the config saves — with
  two new keys in all four locales. Tests: `shares.spec.ts` 51 → **53** and
  `backup.spec.ts` 27 → **28** (a mocked refused action shows the reason in a
  `[data-sonner-toast][data-type="error"]`). Docs:
  [guides/shares.md](docs/guides/shares.md),
  [guides/backup-restic.md](docs/guides/backup-restic.md),
  [guides/storage.md](docs/guides/storage.md).
- **Health — every host probe is bounded (2026-10-24, ROADMAP X.2).** The
  checks run on the blocking pool (and the notification engine runs them every
  60 s), so a `df` parked on a hung NFS mount, an `mdadm --detail` blocked on a
  failing disk or a `smartctl` on a spun-down drive pinned that thread with no
  ceiling. `df`/`lsblk`/`mdadm`/`systemctl` now run under `HOST_PROBE_TIMEOUT`
  (10 s) and `smartctl` under `SMART_PROBE_TIMEOUT` (20 s), through one local
  `run_command` over `utils::process::run_timed`; a timeout or failed spawn
  degrades that check to `Unknown`, never a fabricated healthy reading. Tests:
  `src/health.rs` 17 → **22** (source guard, kill path, exit-status-as-data, a
  missing tool, the probe-budget ordering), `tests/health_integration.rs` 15 →
  **16** (a shim `df` sleeping 250 ms is waited out; `GET /api/health` still
  answers `200`). Docs: [guides/health.md](docs/guides/health.md).
- **Shares/Protocols — every `systemctl`/`exportfs`/`sudo tee` call is bounded
  (2026-10-24, ROADMAP X.2).** The handler's `run_share_op` outer timeout stops
  waiting for the `spawn_blocking` future but does **not** kill the child, so a
  wedged `sudo systemctl` or `exportfs` kept the blocking thread pinned. Every
  call now carries its own ceiling, with `SHARES_WRITE_TIMEOUT` 30 s
  (`sudo tee`/`tee -a`, `sudo mkdir -p`; the two `tee` helpers moved to
  `run_timed_with_stdin`) and `SHARES_SERVICE_TIMEOUT` 60 s (`systemctl`
  start/stop/restart/reload, `exportfs -ra`). Tests: `src/shares.rs` 32 → **37**
  (source guard, kill path, exit-status-as-data, a missing tool, deadline
  ordering); no behavioural integration case by design — every host call here
  is a mutation the suite must not perform on the real host. Docs:
  [guides/shares.md](docs/guides/shares.md).
- **VPN — every `wg`/`wg-quick`/`openvpn`/`pgrep` call is bounded (2026-10-24,
  ROADMAP X.2).** Every command was a bare `Command::output()` (except two
  stdin cases that hand-rolled `.spawn()` + `wait_with_output()`), so a `wg`
  blocked on a missing kernel module, an `openvpn --daemon` parent that never
  exits or a `pgrep` on a huge process table pinned the calling thread with no
  ceiling — and the VPN screen polls the server status and the peer list. All
  call sites now go through `crate::utils::process::run_timed` behind one local
  `run_command(cmd, timeout)`, with `VPN_PROBE_TIMEOUT` 10 s and
  `VPN_WRITE_TIMEOUT` 30 s. The two stdin paths use `run_timed_with_stdin`.
  **New primitive:** the OpenVPN daemon start uses `run_timed_detached`
  (`utils::process`) — `openvpn --daemon`'s daemon keeps the inherited write
  ends of any pipe, so `run_timed`'s drain threads would never see EOF and its
  deadline could not fire; the helper detaches both streams to `/dev/null` and
  only waits for the exit status. Tests: `src/vpn.rs` 29 → **34** (a source
  guard that also requires the detached helper in the daemon path, the kill
  path, exit-status-as-data, a missing tool, the deadline ordering),
  `src/utils/process.rs` 16 → **19** (three `run_timed_detached` cases). Docs:
  [guides/vpn.md](docs/guides/vpn.md).
- **Network — every `ip`/`wpa_cli`/`dhclient`/`netplan`/`ethtool`/`ping`/`ps`
  call is bounded (2026-10-24, ROADMAP X.2).** The module runs on the blocking
  pool (its handlers wrap it in `spawn_blocking`), so a wedged tool did not
  stall just one request: a `sudo wpa_cli` waiting on the root-owned control
  socket, a `dhclient` parked on a dead DHCP server or a `netplan apply`
  blocked on systemd-networkd pinned that blocking thread — and because
  `GET /network/interfaces`, `/wifi/status` and `/summary` are read on every
  view of the Network screen, the calls piled up behind the wedged one with no
  ceiling. All ~38 call sites now go through `crate::utils::process::run_timed`
  behind one local `run_command(cmd, timeout)` helper with three named
  ceilings: `NET_PROBE_TIMEOUT` 10 s (`ip … show`, `wpa_cli status`/
  `list_networks`/`scan_results`, `ethtool`, `sudo cat`, `ps`),
  `NET_WRITE_TIMEOUT` 30 s (`ip addr add|flush|route`, `dhclient`, `wpa_cli`
  connect/select/save, `netplan apply`, `sudo tee`/`chmod`/`rm`) and
  `NET_PING_TIMEOUT` 5 s. The privileged mutations collapsed into
  `ip_sudo_run(program, args)` so a call site cannot forget the `sudo` prefix
  **or** the deadline; `write_as_root` moved to `run_timed_with_stdin` (its
  hand-rolled `.spawn()` + `wait_with_output()` would hang forever on a `tee`
  that never read, and stdin is still written from its own thread); and
  `run_command` keeps a non-zero exit as *data*, which `wpa_cli` refusals rely
  on. Tests: `src/network.rs` 57 → **63** (a source guard — no bare
  `.output()`/`.spawn()`/`.status()`/`wait_with_output()` in production code —
  the kill path, the exit-status-as-data contract, a missing tool, the deadline
  ordering, and the argv shape of both privilege helpers),
  `tests/network_integration.rs` 23 → **24** (a shim `ip` sleeps 250 ms; the
  summary must still come back `200`, after really waiting for it). Docs:
  [guides/network.md](docs/guides/network.md).
- **Cache mover — `rsync`/`find`/`df` are bounded, so a wedged transfer frees
  the single-flight slot (2026-10-24, ROADMAP X.2).** The mover's three host
  tools ran unbounded from inside `spawn_blocking`, and the damage was
  structural rather than local: `run_mover` holds the process-wide
  `MOVER_RUNNING` flag for the whole run, so an `rsync` that never returned kept
  that slot set and **no mover ran again** — from the button or from
  `cache_scheduler` — until a restart. Every call now goes through
  `crate::utils::process::run_timed` behind one `run_command(cmd, timeout)`
  helper with named ceilings: `CACHE_QUERY_TIMEOUT` 30 s (`df -B1`, which also
  gates the free-space check that must answer before `rsync
  --remove-source-files` deletes anything), `CACHE_FIND_TIMEOUT` 600 s (`find`
  sizing a share and building the age list) and `CACHE_RSYNC_TIMEOUT` 6 h (a
  first drain of a full tier legitimately takes hours — a ceiling, like
  rclone's transfer budget). The child is killed at the deadline, so the run
  ends, the RAII guard releases the slot, and the run row records the failure
  with the deadline in its message. The helper keeps a **non-zero exit as
  data**, which the `rsync` path depends on: exit 24 means "vanished source
  files" and is success for the files that did transfer. `encryption.rs` was
  checked in the same pass and needed nothing — its ROADMAP entry was stale,
  every `cryptsetup`/`mount`/`umount`/`tee` call already carries a deadline
  (ROADMAP 9). Tests: `cache.rs` 33 → **38** (a source guard — no bare
  `.output()`/`.spawn()`/`.status()`/`wait_with_output()` in production code —
  the kill path, the exit-status-as-data contract, a missing tool, the deadline
  ordering, and `df_space` answering `None` instead of a fabricated `0`),
  `storage_integration.rs` 29 → **30** (a finished mover run releases its
  single-flight slot, and a second trigger is accepted). Docs:
  [guides/cache.md](docs/guides/cache.md).
- **Storage tools — every `zpool`/`btrfs`/`mdadm`/`smartctl`/`df` call is
  bounded (2026-10-24, ROADMAP X.2).** `disk.rs` bounded its virtual-disk
  *create* path in ROADMAP 8.3 and left every other call bare: the scans the
  RAID/Storage screens run on each poll (`lsblk`, `df`, `mdadm --detail --scan`,
  `mdadm --examine`, `zpool status`, `btrfs filesystem show`, `smartctl
  -A/-H/-c/-l selftest`) and the mutations (`umount`, `--stop`,
  `--zero-superblock`, `rmdir`, `--add`, `--grow`, `--action check`) could hold
  the calling `spawn_blocking` thread forever. `pools.rs` (34 sites) and
  `snapshots.rs` (9 sites) were **entirely** unbounded, because they are
  synchronous. All three modules now go through `crate::utils::process` with
  named ceilings: `POOL_QUERY_TIMEOUT` 30 s / `POOL_WRITE_TIMEOUT` 900 s,
  `SNAPSHOT_QUERY_TIMEOUT` 30 s / `SNAPSHOT_WRITE_TIMEOUT` 600 s, and disk's
  existing `BLKID`/`SMART`/`MDADM`/`MKFS`/`MOUNT`/`ZPOOL`/`INITRAMFS` values.
  `disk.rs` also dropped its private deadline implementation in favour of the
  shared helper, keeping "not installed" (`DiskError::Io`) distinct from "too
  slow" (`DiskError::Timeout`). The stdin trap: persisting a mount point is
  `sudo tee /etc/fstab`, and `run_timed` closes stdin by design — routing that
  through it would make `tee` write an empty file and truncate the target. So
  `utils::process` gained `run_timed_with_stdin` and
  `run_timed_async_with_stdin`, which write the input from their own thread
  (a child that never reads it cannot deadlock the caller before the deadline
  loop starts); `disk.rs`'s hand-rolled spawn and `pools.rs`'s unbounded
  `wait_with_output()` both use them. Tests: `disk.rs` 66 → **70**,
  `pools.rs` 33 → **35**, `snapshots.rs` 4 → **6** (a source guard per module —
  no bare `.output()`/`.spawn()`/`.status()`/`wait_with_output()` in production
  code — plus the deadline-ordering and `bounded` failure-mapping checks),
  `utils/process.rs` 12 → **16**, `storage_integration.rs` 28 → **29** (a shim
  `sudo` that answers in 300 ms must be waited for, not skipped or killed
  early). Docs: [guides/storage.md](docs/guides/storage.md),
  [guides/raid.md](docs/guides/raid.md),
  [guides/snapshots.md](docs/guides/snapshots.md).
- **File Explorer — no blocking work on the async runtime, and every host
  command bounded (2026-10-24, ROADMAP X.1/X.2).** `handlers/explorer.rs` was
  the last module reaching a Tokio worker with blocking work. The disk scan
  (`/proc/mounts`, a `stat` per mount, one `lsblk` pass, one `df -B1` per mount)
  ran as a loop inside the `async fn` — a slow or hung disk parked a worker for
  every open Explorer window; it now runs as `collect_disks()` on the blocking
  pool. The path stats of the download, upload, copy, move, save, roots,
  thumbnail and ticket handlers go through `tokio::fs`
  (`path_exists`/`path_is_file`/`path_is_dir`), `resolve_dest` (which stats and,
  for `keep_both`, probes for a free name) runs on the blocking pool, so do the
  four durability fsyncs and the cleanup sweep of a failed tracked copy/move,
  and the upload-session prune no longer holds its lock across an unlink. Every
  subprocess of the module (and of `src/browser.rs`) now goes through
  `run_timed`: `lsblk`/`df`/`chmod`/`which` at 10 s, archives
  (`tar`/`unzip`/`7z`/`gunzip`) at the new `ARCHIVE_TIMEOUT` (30 min — a ceiling
  for a legitimate first extraction, not an estimate), `sudo umount` at 30 s,
  `sudo zfs list` at 30 s, `wipefs`/`mkfs.*` at 10 min and the neighbour sweep
  at 20 s. Two details worth naming: `run_timed` gained `run_timed_to_file`
  because `gunzip` must write **into** its output file (the plain wrapping would
  have sent the decompressed data to a pipe nobody reads, silently undoing the
  `.gz` streaming rewrite of item 10), and `run_timed` now keeps a caller-set
  stdout instead of overwriting it. Tests: `handlers/explorer.rs` 34 → **38**
  unit (the path helpers, the deadline ordering, and two **source guards** — no
  blocking token in an `async fn` outside a `spawn_blocking` closure, and no
  `.output()`/`.spawn()`/`.status()` in production code), `src/browser.rs` 16 →
  **17**, `src/utils/process.rs` 10 → **12**,
  `tests/explorer_integration.rs` 64 → **65**
  (`test_disk_scan_does_not_block_the_runtime`: a shim `lsblk` sleeping 2 s on
  `PATH`, and a second request that must be served before the scan returns on
  the single-threaded test runtime). Docs:
  [guides/explorer.md](docs/guides/explorer.md).
- **Web shortcuts — the 512 KB cap now bounds the read, and a redirect chain is
  walked hop by hop (2026-10-24, ROADMAP 27.1/27.2).** The metadata fetcher
  awaited `Response::bytes()` and only then sliced the body to
  `MAX_BODY_BYTES`, so a multi-GB response was fully resident in the server's
  memory before the "cap" applied — `read_capped_stream` now stops pulling the
  stream as soon as the cap is reached and drops the rest (the 10 s deadline is
  no longer the only bound). The redirect was followed exactly **once**: a
  second redirect's empty body was parsed as "the page", so metadata silently
  came back empty on every short-link / CDN chain, and `resolve_public`
  validated addresses that reqwest then re-resolved at connect time (a DNS
  answer that changed in between reached an internal address). The chain is now
  walked up to `MAX_REDIRECTS` (5) with each hop joined against the previous
  absolute URL and re-validated (scheme allow-list, host, every resolved
  address) **before** it is dialled; only `301/302/303/307/308` are followed
  (`300/304/305/306` used to be read as "redirect without a Location"), a longer
  chain answers `400 Too many redirects (at most 5 are followed)`, and the
  client resolves through a `PublicOnlyResolver` (`reqwest::dns::Resolve`) so
  the address that is dialled is the one just validated — the rebinding window
  is closed without a second resolution. The same 10 s budget now covers the
  whole chain (send *and* body read) instead of each hop, and one process-wide
  client replaces the per-request one, so reqwest's connection pool, TLS session
  and resolver are reused. Tests: `handlers/shortcuts.rs` 7 → **19** unit
  (streaming reader — the second chunk is never polled once the cap is reached,
  a chunk crossing the cap is cut to its prefix, a broken stream is an error —,
  the followed-status set, relative / protocol-relative / cross-host hops, a
  three-hop chain, per-hop `file:` refusal, the cap message, `parse_url`
  normalisation, and the resolver's private-literal matrix) and
  `shortcuts_integration.rs` 3 → **4**; `shortcuts.spec.ts` 8 → **10** `test(`
  (a refused chain and a timeout each keep the dialog open with the server's
  own sentence, the typed URL, and a re-enabled submit). Docs:
  [guides/shortcuts.md](docs/guides/shortcuts.md), [API.md](docs/API.md).
- **Autostart — the `systemctl` probe left the async runtime, and a refusal
  says why (2026-10-24, ROADMAP 25.1/25.2).** `is_enabled()` / `set_enabled()`
  ran `std::process::Command` straight from `async fn` handlers, so a
  `sudo systemctl enable` waiting on the sudo PAM stack or on a systemd job
  parked a Tokio worker — and, because both calls are flushed by the one
  settings Save button, stalled the whole save. Both now run inside
  `spawn_blocking` with a `run_timed` deadline (`AUTOSTART_PROBE_TIMEOUT` 10 s,
  `AUTOSTART_UPDATE_TIMEOUT` 30 s); the argv is built by a pure `args_for()`, so
  the scoped `systemctl enable|disable homevault` grant is asserted in a unit
  test and the unit name stays a constant, never request input. The second half
  was feedback: the handler mapped **any** failure to
  `settings.autostart_update_failed`, discarding the stderr `set_enabled`
  carefully returned, so “polkit refused”, “a password is required” and “unit
  not found” were indistinguishable. `PUT /settings/autostart` now answers
  `500` with the host's own reason as `message`, which the card renders as a
  dismissible `role="alert"` (`settings.autostart_error`); the staged flip is
  reverted to the last server-accepted value so the switch never claims a
  change the host refused. `GET /settings/autostart` is tri-state: a clean
  non-zero exit is `enabled:false`, but a non-empty stderr (container, no
  systemd, unit missing) is `500` with the reason — “the host cannot answer”
  is no longer reported as “disabled”. A failed read disables the switch
  (`settings.autostart_read_failed`) so it cannot stage a change against an
  unknown baseline. The sudoers grant is unchanged. Tests: `src/autostart.rs`
  0 → **9** unit (exact argv, constant unit name, the three probe states, the
  stderr tail, the silent non-zero exit, the deadlines),
  `settings_core_integration.rs` 24 → **28** (a `PATH` shim stands in for
  `sudo`/`systemctl`: disabled vs unreachable, the refusal reason in the body,
  a successful toggle), `settings.spec.ts` 3.5/3.6 (mocked routes: refusal
  alert + reverted switch, served value on success). **Written but not
  executed** (per instruction). Docs: `docs/guides/settings.md`,
  `docs/API.md`, `ROADMAP.md` (item 25 closed, 26 → 24 open findings),
  `TESTING.md`, and the four `src/i18n/*.toml`.

Also in this change (`ROADMAP X.1`/`X.2`, same module): the two remaining
`std::process::Command` call sites of `handlers/settings.rs` — the
`hostnamectl set-hostname` behind the server-name save and the
`timedatectl set-timezone` next to it — moved to `spawn_blocking` with a 30 s
`run_timed` deadline (`HOSTNAME_APPLY_TIMEOUT` / `TIMEZONE_APPLY_TIMEOUT`), so
no blocking process of that module is reachable from an async handler.

- **Server control — a refused power command is now visible, and every power
  button runs one privileged command (2026-10-24, ROADMAP 24.1/24.2).** The
  desktop power menu's two branches were literally empty
  (`if (response.success) { } else { }`): the confirmation modal closed before
  the call, so a `systemctl` refused for lack of a sudoers grant looked exactly
  like a slow restart — the appliance appeared stuck with no explanation — and
  a successful one said nothing either. The desktop now keeps the request in
  flight (confirm and cancel disabled, so no double submit), then renders a
  dismissible band: `role="status"` (“Restarting the server… the connection
  will drop”) on acceptance, `role="alert"` carrying the server's own words on
  refusal (the power menu stays usable, so retry works). A connection that
  drops before the answer is **not** reported as a refusal — the band says the
  command may still have been accepted, because a box already going down never
  flushes its reply. The Settings → HTTPS card no longer fires and forgets: a
  refused restart is a `role="alert"` on the card, because the new certificate
  is not live until the process restarts. Under that, **the two privilege
  paths were unsound**: `/api/server/restart|shutdown` ran a bare
  `shutdown -r now` (no `sudo`, no deadline) while `/api/power/*` ran
  `sudo systemctl reboot` through `run_timed` — on a normal install the top-bar
  restart always failed while Settings worked. Both families (and the MCP
  `system.reboot` tool, which had a third copy) now call the single
  `power::host_power_action` primitive inside `spawn_blocking`: one
  `sudo systemctl reboot|poweroff|suspend` argv, one 30 s deadline, one
  refusal shape. The sudoers template gains exactly those three verbs
  (`src/setup.rs::power_grants`, built from `PowerAction`), and the bare
  `shutdown` binary is no longer granted or listed as a dependency. The
  `/api/server/*` handlers answer before the box goes down, so a refusal is
  still answerable; `/api/server/*` and `/api/power/*` stay two JSON shapes
  over the same primitive, documented as aliases in `docs/API.md`. Tests:
  `handlers/system.rs` 0 → **4** unit, `power.rs` 5 → **6**, `setup.rs` +1,
  `power_integration.rs` 4 → **9**, new `tests/e2e/desktop/power.spec.ts`
  (**5 `test(` / 10 runtime**, every `POST /api/server/*` mocked — never a real
  reboot). i18n: `desktop.power_restarting|power_shutting_down|power_failed|power_no_answer`
  and `settings.tls_restart_failed` in the four locales. Docs:
  `docs/guides/server-control.md`, `docs/guides/power.md`, `docs/guides/mcp.md`,
  `docs/API.md`, `TESTING.md`, `ROADMAP.md`. **Written but not executed** (per
  instruction): no suite was run.

- **Metrics — a failed read is a gap, not a `0.0`, and a rate is divided by the
  real elapsed time (2026-10-23, ROADMAP 11).** The collector used
  `.unwrap_or(0.0)` for memory, load and CPU while `044_metrics_history.sql`
  declared every column `NOT NULL`, so a transient `/proc` failure wrote a
  plausible-looking `0 %` sample into the chart: a broken sensor, a missing
  `/proc/net/dev` and a genuinely idle disk were the same picture — exactly
  what `docs/guides/metrics.md` promised would never happen. Migration
  `056_metrics_nullable_series.sql` rebuilds `metric_samples` with all five
  measured columns nullable (existing rows copied verbatim, index recreated),
  the collector stores `None` on failure, and each series now averages on its
  own inside a bucket, so a missing thermal zone no longer halves the CPU
  figure sitting next to it. **Rates:** the throughput series divided by the
  constant `SAMPLE_INTERVAL_SECS`, so a sample taken 120 s after the previous
  one reported twice the real rate; `sample_from` now divides by the real
  distance between the two reads, and the loop runs
  `MissedTickBehavior::Skip` instead of the default `Burst` (after a suspend,
  `Burst` fired the missed ticks back-to-back, they collided in the same
  minute bucket and `INSERT OR REPLACE` silently kept only the last one). A
  first sample after a boot, a counter reset and a frozen clock store **no
  rate** instead of a zero, and the three redundant `/proc` reads per tick
  became one read per file. `GET /metrics/latest` answers `200` +
  `data: null` on a fresh install instead of `400 "No metrics collected yet"`
  — "nothing measured yet" is a first-boot state, not a bad request.
  **Frontend:** the empty state is decided on all six series (a
  temperature-only dataset used to render "no data"), every `load()` takes a
  ticket so a slow `24h` answer cannot repaint a newer `1h` range, and a
  missing figure prints `—` instead of `0.0`. Tests: `metrics.rs` 5 → **19**,
  `metrics_integration.rs` 5 → **8**, `metrics.spec.ts` 4 → **6** `test(`.
  Written on 2026-10-23; **not run yet** (per instruction).
  `docs/guides/metrics.md` gained *Null semantics* and *Rates divide by the
  real elapsed time*; `docs/API.md` documents the `data: null` answer.

- **Firewall/ufw production-ready pass (2026-10-21) — nothing unvalidated
  reaches `sudo ufw`, and every destructive action is confirmed and verified.**

  Seven holes, the first three of them reachable from a plain admin session:

  1. **Client input was interpolated straight into the `ufw` argv.** `action`
     is the *verb* of `sudo ufw <action> …`, so `{"action":"delete"}` on the
     "add rule" endpoint ran `sudo ufw delete …` (`--force`, `reset` and
     `enable` were equally reachable). `protocol`, `port` and `source` went
     through as-is: a port of `22/tcp` was concatenated a second time
     (`22/tcp/tcp`), and `source: "any"` turned a "restricted" rule into a
     global one. New `validate_action` / `validate_protocol` / `validate_port`
     / `validate_source` / `validate_comment` plus the default-policy pair, all
     pure and unit-tested, run **before** the `ufw_available()` probe — a typo
     is reported as a typo, and no process is spawned.
  2. **A source-restricted rule could not be deleted, and the global rule with
     the same port was deleted instead.** `delete_rule_by_spec` rebuilt
     `delete <action> <port>/<proto>`, which does not match `allow from <cidr>
     to any port <n> proto <tcp>`. `FirewallRule` now carries `spec` — the
     exact argument string `ufw show added` printed, comment stripped — and the
     handler passes *that* to `ufw delete`. The client only selects a live
     rule; it never supplies the command.
  3. **The mandatory-rule protection was bypassable.** The target was looked up
     ignoring `source`, and when nothing matched the handler deleted anyway — so
     a raw call could remove the SSH/app-port rule (lockout) or a rule for
     another port. A miss is now a **404** and the matched rule's own live
     `is_mandatory` flag a **403**.
  4. **Enabling trusted an exit code that lies.** `ufw --force enable` exits 0
     even when iptables failed to load ("ERROR: problem running iptables"), so
     `enabled = true` was recorded on a firewall filtering nothing.
     `enable_verified` re-reads the live state and, if a mandatory allow rule is
     missing, turns the firewall back off and explains why instead of leaving
     the appliance unreachable.
  5. **Disabling had no confirmation** — the only one-click destructive action
     on the page, and it opens every port the firewall blocked. It now requires
     the same `{"confirmed": true}` body as enabling.
  6. **A `Port 2222` in `/etc/ssh/sshd_config.d/*.conf` was invisible**, so the
     mandatory rule protected port 22 while SSH listened on 2222 — a guaranteed
     lockout on the first enable. `detect_ssh_ports()` reads the main file
     **and** the drop-ins and keeps every `Port` value.
  7. **Every `ufw` call was a blocking `std::process::Command` in an async
     handler, with no timeout.** `ufw` shells out to `iptables`, which takes the
     xtables lock: a wedged call parked a Tokio worker for the life of the
     process. All calls now go through `tokio::process::Command` with
     `kill_on_drop` and a 30 s `UFW_TIMEOUT`.

  Also fixed: the rule list mis-parsed interface-scoped rules (`in on eth0`) and
  rules with no port (`allow from 1.2.3.4`), and could read a keyword out of a
  rule's own comment; a duplicate rule was reported and audited as a fresh
  change although ufw skips it and exits 0; `disable` wrote back a cached
  `ssh_port` of 0.

  **Added:** `GET /firewall/status` returns `default_policy` (parsed from
  `ufw status verbose`), new admin `POST /firewall/default-policy`, the enable
  preview **warns when the incoming default is `allow`** (enabling would then
  filter nothing), and `GET /firewall/history` exposes the change log the
  handlers had always recorded and nothing ever read (`list_change_log` was
  dead code) with a new `default_policy_changed` action. The card gains the
  default-policy row and dialog, a "Recent changes" list, one focus-trapped
  confirmation dialog shared by enable/disable/remove/policy (it was a
  `role="presentation"` div inside a `role="button"` overlay with no focus
  handling), labelled inputs, an `aria-label` per row's remove button, and a
  `role="status"` line reporting every outcome — failures used to land in an
  empty `else {}`.

  Tests: `src/firewall.rs` **9 → 31** unit, `src/handlers/firewall.rs` **0 → 2**,
  `security_integration.rs` **18 → 25** (both confirmation gates, the pre-ufw
  rejections, the 404/403 delete paths, the history envelope),
  `security.spec.ts` **30 → 41** `test(` blocks (39 → 43 runtime tests) — the
  new §5b intercepts the firewall endpoints with `page.route`, so the card's
  default policy, focus-trapped remove dialog, `spec` payload, disable gate and
  error feedback are covered on a host where `sudo ufw` wants a password.
  Verified: `cargo test --lib firewall` 31/31,
  `cargo test --test security_integration` 25/25, `security.spec.ts` 35 passed
  / 8 skipped / 0 failed (Firefox, scratch server `:8102`), `npm run check`
  0/0, `npm run build` OK. No live `ufw` mutation: `sudo -n true` fails on this
  machine, so `ufw_available()` is `false` and the live enable/disable cycle
  stays documented from the 2026-07-16 signed-off audit.

- **The E2E suites never re-checked the first-sign-in password gate they walked
  into (2026-10-21).** `middleware::password_change_guard` (added with the
  Users/Groups pass) answers `403 password_change_required` to *every* API call
  from an account still holding an admin-chosen password. Fourteen E2E specs
  built a throwaway account with `POST /api/users` and then used the returned
  session directly, so six specs went red (`chat` 4.3, `cloudsync` 4.3,
  `downloader` 10.3, `storage` 4.4, `syncthing` 3.3, `mcp` 15.5) and eight more
  turned vacuous — the "a non-admin is refused" assertions in `vpn`, `network`,
  `shares`, `rescue`, `explorer`, `raid` and `security` 6.3 got their 403 from
  the forced-change gate instead of the admin gate, and `login` 24.2/24.3
  silently skipped because the recovery-code generation was refused, dropping
  the only end-to-end cover of the recovery-code reset. Every creator helper now
  clears the flag before the session is used (log in → `PUT
  /api/users/{u}/password` with the current password → log in again, since the
  change revokes the flagged session) and returns the owner-chosen password;
  `login` 23.1 asserts the real ordering (change screen first, not-admin
  refusal after) and 24.x clears the flag before generating the code. Two
  spec-level bugs surfaced by the same re-run: `account.spec.ts` dismisses the
  once-per-version "What's New" modal (its overlay intercepted the user-menu
  click on every brand-new account), and `login` 25.3 retries until the lock
  copy appears instead of assuming the fifth submit is the one that locks (the
  per-IP throttle is shared by the whole suite; the five-attempt threshold stays
  pinned by the Rust suite). No production code changed.

- **Backup/restic production-ready pass (2026-10-21) — the run is a real
  background job, its progress is real, and no unvalidated value reaches
  restic's argv.**

  Five user-visible holes:

  1. **Backup and restore ran inside the HTTP request.** `POST /restic/backup`
     and `/restic/restore` blocked until restic exited — hours on a multi-TB
     dataset. The browser saw a stalled request, the floating island only
     learned about the job once it had already finished (so it showed a
     *completed* backup from the start), and a Tokio worker stayed parked.
     Both endpoints now answer immediately with
     `{status:"running", job_id, backup_job_id}` and the run continues in a
     spawned task, the blocking restic call on `spawn_blocking`
     (`ResticService::run_restic_op`). Snapshots, stats, unlock, prune, forget
     and restore-plan go through the same helper with a timeout
     (`RESTIC_QUERY_TIMEOUT` 120 s, `RESTIC_INIT_TIMEOUT` 300 s for `init`).
  2. **The progress bar was fabricated** (5 %, then 90 %). `restic backup
     --json` is newline-delimited JSON; it is now read line by line
     (`create_backup_streaming`) and `percent_done` / `bytes_done` drive real
     SSE progress (`ResticProgress`), mapped to 1–99 % so 100 % waits for the
     summary line.
  3. **Every healthy repository was reported broken.** `check_repo` collected
     any line containing `error`, and `restic check` prints "**no errors were
     found**" on success — so `healthy` was `false` for a healthy repository.
     The verdict now comes from `parse_check_output`, which excludes the
     success sentinel and only counts `error:` / `fatal:` / `unable to` /
     `failed to` / `is damaged`.
  4. **Argument injection could wipe a repository.** `snapshot_id` was passed
     positionally to `restic forget`, so `{"snapshot_id":"--keep-last=0"}` was
     parsed as an option and forgot **every** snapshot. `validate_snapshot_id`
     (`[A-Za-z0-9._-]`, no leading `-`) plus absolute-path validation of
     backup sources and restore targets, a strict pre-backup snapshot name
     (it becomes `/tmp/restic-snap-<name>`), and `validate_repository` /
     `validate_password` / `validate_display_name` / `validate_keep_within` /
     `validate_tags` / `validate_exclude_patterns` refuse the value before
     restic is spawned. Repository create/update also refuse an empty name,
     location or password, a relative source path, an unknown
     `retention_policy_id`, and `snapshot_create_before_backup` without a pool
     and an absolute dataset.
  5. **A failed snapshot mount was ignored**, so the backup captured an empty
     mount point, reported success and deleted the snapshot. `SnapshotMount`
     (RAII) now aborts the run when the mount fails.

  The run is supervised too: **one backup/restore per repository**
  (`RepoRunLock`, shared with the scheduler, released on `Drop`) answers
  "already running" instead of restic's "repository is already locked" after a
  full scan; **cancellation really stops the process** (SIGINT so restic
  releases its lock, SIGKILL after 10 s; the job ends `cancelled`); the
  `restic_backup_jobs` row is written **before** the run and
  `fail_stale_running_jobs()` closes leftovers at startup (a crash used to
  leave a phantom `running` job for ever), with `snapshot_id` filled in on
  completion; **`max_size_gb` is finally applied** — it was stored, returned
  by the API and never read, and now becomes a `--keep-last` derived from the
  average snapshot size (`plan_keep_last_for_size`), never 0 and never
  deleting the newest snapshot; **retention policies are validated** (limits
  > 0, at least one limit on create, a single `apply_to_all` default); and
  **`check --read-data` is admin** (it re-reads every pack). Frontend:
  `trackJob(jobId, label, onSettle?)` refreshes the lists when the job really
  settles, the policy form gained the size cap, and "Apply retention" is
  disabled when no bucket is set.

  Tests: `restic.rs` 8 → 29, `restic_scheduler.rs` 12 → 16,
  `handlers/restic.rs` 3 → 6, `restic_integration.rs` 17 → 24,
  `backup.spec.ts` 25 → 27. `cargo test --lib restic` 51/51,
  `cargo test --test restic_integration` 24/24, `npm run check` 0/0,
  `npm run build` OK, `cargo clippy --all-targets` clean on the touched files.
  The §13 real-restic E2E skips on this host (`restic` is not installed).

- **Users/Groups production-ready pass (2026-10-21) — deleting an account now
  really removes it, and demoting an admin really demotes it.**

  A HomeVault account can be backed by a real Linux account (a PAM sign-in
  mirrors one; an admin promotion provisions one). Nothing ever touched that
  host half, which produced four holes:

  1. **`DELETE /users/{u}` left a working login.** Only the row was dropped, so
     a deleted admin kept its Linux login *and* — admin status is derived from
     `homevault_admin` membership — a working HomeVault **admin** login that
     the next PAM sign-in silently recreated. The handler now runs `userdel`
     **before** the row delete, so a refusal (account still logged in, no
     privileges) aborts with a clear message and leaves both sides consistent
     and retryable. A PAM-mirrored row whose Linux account still exists is
     refused (`400`, message names the command) because HomeVault did not
     create that account; if the Linux account is already gone the stale row
     *can* be removed, so the page stays cleanable.
  2. **Demotion left real sudo.** `PUT /users/{u}/role` cleared only
     `homevault_admin`; `sudo`/`wheel` stayed, and the demoted session kept the
     `is_admin` flag it was created with (up to 30 days with "remember me").
     `HostAccountService::revoke_admin` clears all three **before** the
     `role_id` write, `AuthService::update_role` signs the demoted account out,
     and `AuthService::get_session` re-derives `is_admin` from the current role
     on every call.
  3. **Locking left the host password usable** — SSH and the web terminal
     ignored the DB flag. `usermod -L` / `-U` now mirrors lock/unlock.
  4. **A password reset was a no-op for a PAM-mirrored account** (its hash is
     the PAM sentinel, so the old host password kept working). The new secret
     is now pushed through `chpasswd` on **stdin** (never argv, which `ps`
     would expose).

  New module `src/host_accounts.rs` (`HostAccountService`) is the single entry
  point for all of it: a username gate before any command
  (`[A-Za-z_][A-Za-z0-9_-]{0,31}`, never a leading `-`), a system-account
  refusal (`uid < 1000` or `>= 65534`), a 20 s `HOST_OP_TIMEOUT` on
  `tokio::process` with `kill_on_drop`, and `userdel` deliberately without
  `-r` (home removal follows the explicit `delete_home_folder` choice and the
  path HomeVault recorded). The Users page shows a **Linux account** badge on
  mirrored rows and disables Delete with an explanation (4 locales), and the
  home-configuration row is now dropped with the account even when the files
  are kept, so a recreated username inherits neither path nor quota.

  Groups hardening in the same pass: a group member must be an existing
  HomeVault user (`create_group`/`add_user_to_group` provision a Linux account
  per member, so an unchecked name created an unmanaged OS account) — blank
  names and >500 members are refused too; `DELETE /groups/{name}` now purges
  **all four** display-name references (`hv_groups`, `transfer_quotas`,
  `resource_group_shares` and — new — `share_permissions`, extracted as the
  testable `purge_group_references`), matching what a rename migrates;
  `GET /users/{u}/groups` is admin-or-self; and every handler call into
  `GroupService` now goes through `crate::groups::run_group_op`
  (`spawn_blocking`, 15 s ceiling) instead of blocking a Tokio worker — the
  Users list resolves every row's `id -nG` in one hop instead of one per row.

  Tests: `host_accounts` unit 14/14; `users_integration` 71 → 79;
  `groups_integration` 34 → 38; `groups.rs` unit 5 → 9; `users.spec.ts` +5.10b.
  The six real host effects are `#[ignore]`d root-only tests in
  `tests/login_pam_integration.rs` (see the TESTING.md checklist) — they were
  not run here (no disposable host account on the workstation).

- **Rescue production-ready pass (2026-10-21).** Four classes of gap in the one
  app allowed to write to raw block devices:

  1. **The Cancel button cancelled nothing.** `DELETE /api/progress/{job_id}`
     removed the job from the tracking map and answered "Job cancelled" while
     the `sudo ddrescue` clone kept writing to the destination disk (and
     `photorec` kept carving) — nothing in the process ever observed the
     request, and the UI had no stop control at all. `ProgressManager` now has
     `register_cancel_hook` / `clear_cancel_hook` / `cancel_job`
     (`CancelOutcome::{NotFound,NoHook,HookRan}`) and `mark_cancelled`, using
     the `ProgressState::Cancelled` that already existed but was never emitted;
     the handler runs the producer's hook and answers
     `{job_id, cancelled, message}` (`400` for an unknown id, instead of a
     phantom success). New `src/rescue/jobs.rs` tracks each rescue job that owns
     a live process: both tools are spawned with `process_group(0)` and a cancel
     runs `sudo kill -s TERM -- -<pgid>`, escalating to `SIGKILL` after 10 s —
     signalling only the `sudo` wrapper left the real tool running. A cancel
     that arrives between `register` and `spawn` is not lost. A cancelled clone
     is reported as *cancelled* (not failed, not completed — it has copied only
     part of the disk) and keeps its mapfile so it can be resumed.
  2. **Arbitrary-file read.** `GET /api/rescue/ddrescue/status/{logfile}` took
     the raw URL segment and `read_to_string`'d it as the service user, so any
     admin session could read (and probe the existence and size of) any file.
     `validate_readable_log_path` now requires an existing regular file under
     `/tmp`, `/var/tmp` or `paths::rescue_dir()`, with no symlink in the final
     component (a planted `/tmp/x -> /etc/shadow` must not be read through) and
     ≤ 16 MiB; a missing file still means "no mapfile yet". The same
     non-following check and component-wise containment (`/tmpfoo` is not inside
     `/tmp`) applies to output paths, and a symlinked parent resolving outside
     the allowed roots is refused.
  3. **Destructive clone with no safety net.** `check_clone_safety` (pure,
     unit-tested) now refuses a destination that is the same device, is in use
     (`lsblk` mountpoints on the device *and its children* plus sysfs `holders`,
     which catches a member of an active md array / LVM VG / dm-crypt mapping
     that has no mountpoint of its own) or is smaller than the source —
     unreadable sizes degrade to a warning. `validate_block_device` resolves
     the **live** root disk through `findmnt /` instead of trusting a hardcoded
     `sda`/`nvme0n1`/`vda` list, and matches device names partition-aware
     (`sda1` belongs to `sda`; `sdaa` does not).
  4. **Options that lied.** TestDisk's `deep_search`/`log_path`/`force` and
     PhotoRec's `keep_broken`/`max_size` were accepted, shown as applied in the
     UI, and never reached the tool; `mdadm`'s `action` was forwarded verbatim
     into `--action=`; `file_types` were spliced into PhotoRec's
     **comma-separated** `/cmd` grammar (so `"jpg,search"` injected a command);
     no subprocess except the TestDisk scan had a timeout. Every option is now
     validated (`DdrescueOptions::validate`, `PhotoRecOptions::validate`,
     `MdadmOptions::validated_action`) and an un-honourable field is a `400`
     naming it; the dead fields are gone from the API *and* the UI (TestDisk has
     no advanced block any more — nothing left to put in one). All subprocesses
     run through `RescueService::run_output` (kill-on-drop + timeout: probes
     10 s, `lsblk` 15 s, TestDisk analyse 120 s, TestDisk scan 300 s, mdadm
     60 s).
  5. **Root-owned output into a world-writable directory.** The default
     ddrescue mapfile was `/tmp/ddrescue.log` — shared between runs (a second
     clone read the first run's map and silently copied nothing while reporting
     success) and written by a root tool in a directory anyone can plant a
     symlink in; it is now per-job under the root-owned
     `paths::rescue_dir()`, with a `GET /api/rescue/ddrescue/mapfile` route
     reporting it. PhotoRec's recovered tree is `chown -R`'d to the requesting
     admin (the carve runs as root, so the files were otherwise unreadable to
     them) and the frontend no longer discards the recovered path it parsed out
     of the completion message.

  Tests: `rescue` unit 8 → **44**, `sse::progress` unit 20 → **26**, `lib`
  965 → **999**, `rescue_integration` 28 → **38**, `progress_integration`
  7 → **8**, E2E `rescue.spec.ts` 53 → **59**.

- **Shares production-ready pass (2026-10-21).** Share fields were formatted
  verbatim into **root-owned** config files, so every one of them was a config
  injection: `name`/`comment`/`path` into `/etc/samba/smb.conf` (a newline in a
  name forged an extra Samba section — `include = /tmp/evil.conf` — and a
  bracket broke out of the section header), `path` + `options` into
  `/etc/exports` (whitespace shifted the columns, a `)` in an option closed the
  client list and fabricated `*(no_root_squash)`), `welcome_message` into
  `vsftpd.conf` (`ftpd_banner=` is one line) and the SFTP jail path pair into
  `/etc/fstab`. Pure validators (`validate_share_name`,
  `validate_share_comment`, `validate_share_path`, `validate_nfs_options`,
  `validate_smb_config`, `validate_ftp_config`) now run inside `SharesService`
  before any privileged write — so handler, cache-share sync and
  ransomware auto-disable paths are all covered — and reject with a `400`
  naming the field. `/etc/exports` is rewritten through one parser/generator
  pair instead of ad hoc line surgery: the NFS toggle writes a
  `# homevault-nfs-disabled:<name>` tag (the old
  `exportfs -o rw /<name>` / `exportfs -u /<name>` calls were a wrong-shape
  no-op that only changed the in-memory table and silently came back on
  reload/reboot), and delete/update resolve the single matching line by parsed
  name — two exports whose path basenames collided used to delete each other
  silently, they now answer `400` listing both paths. Every `SharesService`
  call moved off the async runtime (`spawn_blocking` + 30 s timeout, SFTP jail
  rebuild 120 s with its DB phase kept async and its mount/account phase
  blocking), and `rebuild_jails` / `list_grantees` / `set_authorized_key` no
  longer run `sudo` inline in a handler. Tests: `shares.rs` unit 14 → 34,
  `sftp.rs` unit 6 → 9, `shares_integration.rs` 20 → 26. See
  `docs/guides/shares.md` and `CLAUDE.md` (2026-10-21 addendum).
- **VPN production-ready pass (2026-10-21).** Three gap classes closed,
  verified empirically against OpenVPN 2.6.19 during the pass:
  (a) an **OpenVPN start now verifies the tunnel before answering** —
  `openvpn --daemon` exits 0 immediately, so starts used to report success
  while the daemon silently retried forever (bad credentials, unreachable
  endpoint); the daemon now launches with a per-connection log and
  `--cd /etc/openvpn` (`--daemon` chdir()s to `/`, which used to break every
  relative path inside a profile; the client-side `--status` file carries no
  connection state, only statistics — both verified on a live 2.6.19 client),
  the log is polled 250 ms for up to 10 s for `Initialization Sequence
  Completed`, a fatal (`Options error` / `Exiting due to fatal error`)
  short-circuits the wait, and on timeout *or* fatal the daemon is killed
  (rollback) and the API answers naming the log's error line. Starting an
  already-running profile is idempotent. Credentials live in
  `/etc/openvpn/openvpn-<name>.auth` (0600), are **kept while the connection
  runs** (OpenVPN re-reads them on renegotiation) and are deleted on
  stop/delete — they were previously thrown into `/run`, which the
  unprivileged service cannot even write; (b) **uploaded `.ovpn` profiles
  are content-validated** — the exec-hook directives (`up`, `down`,
  `down-pre`, `route-up`, `ipchange`, `learn-address`, `client-connect`,
  `client-disconnect`, `auth-user-pass-verify`, `tls-verify`, `askpass`) and
  `script-security > 1` are refused (400 naming the directive, enforced both
  at engine save time and at the handler), and re-uploading over a profile
  whose daemon is currently running answers **409** (stop first);
  (c) `ensure_wireguard_server` verifies `wg0` actually came up after
  `wg-quick up` instead of trusting its silent success. Plus: every VPN
  handler mutation runs through `spawn_blocking` (a start may hold the full
  10 s grace), stop/delete remove the per-connection `.log`/`.auth`
  defence files, start failures are `log_event`-warned, and
  `discover_public_ip` gets a **`PublicIpCache`** (15 min positive / 60 s
  negative TTL) so VPN status polls and peer provisioning no longer mean an
  external HTTPS request per page view. Frontend: the OpenVPN card's start
  button shows a disabled spinning connecting state while the verified start
  is in flight (`vpn.connecting` ×4 locales). `docs/guides/vpn.md` rewritten
  (the previous text still described the pre-roadwarrior
  `/wireguard/keygen` + `/{name}/start` endpoints) and `docs/API.md`'s VPN
  table now lists the real routes. Tests: `vpn.rs` unit 14 → 29 (deny-list
  matrix, log parsers, summary stripping), `vpn_integration.rs` 22 → 25
  (guarded content-validation 400s). See `CLAUDE.md` (2026-10-21 addendum)
  and `TESTING.md` remaining list for the live tunnel pass.
- **App-proxy auth under HTTPS (2026-10-21).** The proxy gate redirected
  unauthenticated browsers to a hard-coded `http://{main_port}/app-auth`; on a
  TLS deployment the `hv_session` cookie carries `Secure` and the browser
  would never re-send it over plain HTTP, looping the app-auth round-trip
  forever and making every "require login" app unusable exactly on the
  high-security setups that want the gate. The redirect now targets
  `https://{host}:{https_port}/app-auth` whenever the HTTPS listener is up
  (computed per request — TLS can be enabled after a proxy spawned), and the
  auth-minting redirects (gate challenge + `/app-auth`) carry
  `Cache-Control: no-store`. The login screen honours the flow's
  `next=/app-auth?…` parameter — same-origin paths only, so no open-redirect
  window — and continues the round-trip after sign-in instead of dead-dropping
  on the desktop. `hv_app_session` deliberately stays without `Secure` (the
  proxy listener is plain HTTP even under an HTTPS dashboard). Also: the
  in-memory app-session store is now capped at 512 live entries with
  soonest-expiring eviction, so repeated `/app-auth` hits cannot grow it
  without limit. `docs/guides/apps-center.md` rewritten to match the shipped
  launcher (Docker apps open in a new browser tab; the old iframe +
  mixed-content-toast design was never on this path). Tests: `serve.rs` 4 unit
  (TLS-aware location builder), `app_auth.rs` +1 (store cap),
  `tests/app_proxy_integration.rs` step 7 asserts the TLS redirect end-to-end.

- **Terminal E2E suite (2026-10-20).** After the `@xterm/xterm` 5 → 6
  dependency refresh of 2026-10-19, `terminal.spec.ts` re-runs green on
  firefox and chromium (81/81 each): the scratch-instance "What's New" modal
  is dismissed before driving the dock, 2.10 compares the terminal body
  against the v6 paint surface (`.xterm-scrollable-element`) instead of the
  v5-era `.xterm-viewport` (which is now a hard-coded black lib layer), 14.1
  asserts survival invariants instead of an exact tab count, and the
  dev-server-only failures (4.2, 10.3) pass against the production bundle.
- **RAID validation order + four HEAD-red unit tests (2026-10-20).** The
  2026-10-19 "20 findings" pass was never test-executed; `cargo test` at HEAD
  failed on 4 unit tests. `validate_virtual_disk_request` now applies the
  raid10 even-count rules to `md_member_slots` **before** the device-count
  floor, and the floor no longer shades the degraded-create semantics
  (2-member raid5 `allow_missing` is valid — the `missing` placeholder fills
  to the exact minimum; the budget governs); the health order test asserts the
  nine checks including `raid_arrays`; the initramfs best-effort fallbacks
  (`update-initramfs`, `dracut`) are pinned in a documented
  `BEST_EFFORT_INITRAMFS_TOOLS` audit exemption instead of being flagged as
  uninstallable. Full `cargo test` green (lib 892, all integration suites).

### Changed

- **Appearance — one appliance look every account can read, a validated
  wallpaper, and two no-op fields deleted (2026-10-24, ROADMAP 26.1/26.2).**
  **A non-admin never received the appliance's appearance.** The desktop's boot
  hydration called `GET /api/settings`, which is admin-only, so for every
  non-admin the request failed, the `catch` swallowed it and the desktop kept
  whatever that origin's `localStorage` held: the same account on two addresses
  showed two wallpapers, with no explanation, and a non-admin on a fresh browser
  got the default even though the appliance had one saved. Decision (recorded in
  `docs/guides/settings.md`): appearance is a **single, appliance-wide** value —
  wallpaper + accent colour — written by an admin and **read by any signed-in
  session**; it is not per-account. `GET /api/settings/appearance` is now
  session-readable and returns **exactly** `{ wallpaper, accent_color }` — no
  e-mail, SMTP, webhook, update or notification field, so widening it leaks
  nothing (the aggregate `GET /settings` stays admin-only, and the read is still
  `400`/`401` without a session). The hydration calls it, and a failure is
  reported **once** (a toast) with the desktop keeping its last known value, as
  an explained fallback instead of a silent one. `PUT` stays admin-only, so the
  Settings → Appearance grid is now rendered **read-only** for a non-admin
  (`data-readonly`, every tile `disabled`, "Managed by the administrator") —
  never a control that appears to save and does nothing. **The wallpaper is
  validated server-side.** It was stored verbatim as a JSON string inside the
  settings blob and parsed by two frontend readers behind a `try/catch`, so one
  malformed value silently reset the desktop. `validate_wallpaper_field` now
  accepts `""`, `/wallpapers/hv-NN.webp`, `hv-NN`, a bare JSON string or the
  `WallpaperConfig` JSON (`{"type":"image","value":…}`; the legacy
  `{"type":"default"}` maps to `hv-01`), normalises everything to the bare URL
  and bounds N to 1–44; anything else (a custom upload, a data URL, a gradient,
  another host) is a `400` whose message starts `wallpaper:`, with the stored
  value untouched (both fields are validated before the first write). The same
  rule runs on load, so a hand-edited `settings.json` cannot inject a value
  either. `validate_accent_color_field` restricts the accent to
  `#rgb`/`#rrggbb`/`#rrggbbaa`, closing a CSS-injection vector into
  `--hv-primary`. **`theme` and `density` are gone.** Both were persisted and
  round-tripped but consumed by nothing (`grep -rn "data-theme\|data-density"
  frontend/src` → 0 hits; the tree has no dark theme), so a user could save
  "dark"/"compact" and see nothing change, forever. They are removed from the
  model, both appearance handlers, the aggregate, the Settings draft, the API
  type and all four i18n files (`settings.density` dropped, keeping key parity);
  an older `data/settings.json` still loads — the two keys are read for
  backward compatibility and ignored. `docs/guides/themes.md`, which documented
  a `ThemeToggle.svelte` and a theme registry that never existed, is rewritten
  to describe the shipped look. Tests: `handlers/settings.rs` 25 → **29**
  unit, `settings_persistence.rs` 13 → **15**, `src/i18n.rs` 8 → **9**
  (key parity), `settings_core_integration.rs` 24 → **28** (the non-admin read
  is the assertion that would have caught 26.1; a non-admin `PUT` mutates
  nothing; a malformed wallpaper is a `400` naming the field with the stored
  value unchanged), `settings.spec.ts` 104 → **111 `test(`** (a non-admin
  desktop paints the server accent although `localStorage` says otherwise, the
  grid is read-only with its reason, and the Save PUT carries exactly the two
  real fields). **Written, not run in this pass** (per instruction); `cargo
  check --all-targets` clean, `svelte-check` 0/0, `npm run build` OK.

- **The 7 simplicity rules are now project rules (2026-10-20).** HomeVault is
  an appliance for a home user, so simplicity is a correctness bar next to
  accessibility: (1) immediately understandable, (2) only what the task needs,
  (3) fewer steps / less effort, (4) progressive complexity, (5) predictable
  and consistent, (6) reassuring and reversible, (7) adapted to the user.
  Written into `CLAUDE.md` → "Simplicity rules" (binding, with the
  accessibility baseline taking precedence where they interact), summarized
  for every agent in `AGENTS.md` → "Simplicity is a correctness bar", and
  detailed with the per-screen review checklist in
  `docs/guides/design-system.md` → §7.2. No code change: this is the review
  bar every new or edited screen is now held to.

- **Workspace hygiene audit (2026-10-19).** Small correctness-of-the-tree fixes
  from a full verification pass (`cargo test` 1 622/0, `svelte-check` 0/0,
  `npm run build` OK):
  - **Docs**: `docs/BACKEND.md` now lists migrations **001–048** with their
    purpose (was 001–038), and `CLAUDE.md` points at the same latest
    migration — both counts were silently stale since the MCP/others
    migrations landed.
  - **Dead theme CSS removed**: `.hv-modal-overlay`, `.hv-modal-dialog`,
    `.hv-modal-header`, `.hv-modal-close` (+ `:hover`), `.hv-modal-content`
    and `.hv-modal-footer` matched no element in any of the 132 `.svelte`
    files of `frontend/src` (the two classes flagged by the 2026-09-17 audit
    plus four more found by the same DOM-level re-check); the three tokens
    only they consumed went with them (`--hv-modal-max-width`,
    `--hv-modal-header-padding`, `--hv-modal-close-size`). Kept:
    `.hv-modal-title` and the overlay/shadow/backdrop **tokens**
    (Drawer/SearchBar/FirewallCard consume the tokens, not the class).
  - **`Explorer.svelte`**: the file-area `div`'s `onmousedown` (marquee
    selection start) is now annotated `svelte-ignore
    a11y_no_noninteractive_element_interactions` with the justification —
    it is a pointer-only enhancement on top of rows/buttons that keep
    keyboard selection; `svelte-check` reports 0 warnings again.
  - **`frontend/.gitignore`**: the `.svelte-check/` cache directory is
    ignored (svelte-check's native-tsgo mode writes it at the repo root).

- **Dependency refresh (2026-10-19).** Every direct and transitive dependency
  was moved to its latest stable release.
  - **Rust**: `Cargo.lock` refreshed (hyper 1.12, tokio 1.53.3, serde_json
    1.0.154, uuid 1.28, toml 1.1.8, libc 0.2.191, rustls 0.23.46, …), and
    `aes-gcm` 0.10 → 0.11 (the `Aead`/`Nonce` API used by
    `storage/user_2fa_repo.rs` is unchanged, so no call-site edit was
    needed). `tokio-tungstenite` stays on 0.29 on purpose: axum 0.8 still
    requires `^0.29`, and bumping it would link two copies of tungstenite
    into the appliance binary.
  - **Frontend**: all packages updated (`svelte` 5.57.2, `vite` 8.3.4,
    `@lucide/svelte` 1.59, `layerchart` 2.8.1, `tailwindcss` 4.3.3, …).
    The deprecated `xterm` / `xterm-addon-fit` / `xterm-addon-web-links`
    packages were replaced by `@xterm/xterm` 6.0.0 / `@xterm/addon-fit`
    0.11 / `@xterm/addon-web-links` 0.12 (imports and CSS path updated in
    `routes/Terminal.svelte` and `components/vm/VmConsole.svelte`).
    TypeScript is now the native 7.0.2 compiler, which `svelte-check` only
    supports through its `--tsgo-experimental-api` flag and alongside
    TypeScript 6 — hence the `typescript@~6` + `@typescript/native` alias
    pair and the updated `npm run check` script.
  - **Security**: `npm audit` is clean (the `devalue` 5.9.2 advisory chain
    pulled in by Svelte is fixed by 5.9.4).
  - **Tooling**: `@playwright/test` 1.64.1 and the matching
    `mcr.microsoft.com/playwright:v1.64.1-noble` image; GitHub Actions
    `checkout@v7`, `setup-node@v7`, `upload-artifact@v7` and
    `gitleaks-action@v3`; CI Node 22 → 24 (current LTS) in all workflows.
  - **Catalog images**: `postgres:18-alpine`, `redis:8-alpine`,
    `tensorchord/pgvecto-rs:pg16-v0.3.0`, `louislam/uptime-kuma:2`.
  - **Dependabot** now also tracks GitHub Actions, the root npm package and
    the catalog `docker-compose.yml` files.

### Fixed

- **Ollama never installed on a fresh host: `zstd` was missing.** Ollama
  publishes `ollama-linux-<arch>.tar.zst`, and its official installer aborts
  with "This version requires zstd for extraction" when `zstd` is absent.
  `zstd` is Priority: optional, so a fresh Debian/Ubuntu does not have it —
  the installer's `curl … | sh` failed, the warning was easy to miss, and the
  AI assistant stayed offline on every new deployment. `install.sh` now
  installs `zstd` before running Ollama's script and checks it explicitly.

- **Docker Compose could abort the whole installation, or leave apt broken.**
  The Docker repository was added with an unguarded `curl … | gpg --dearmor`
  and an unguarded `apt-get update -qq`; under `set -euo pipefail` a
  transient network failure stopped the installer at Step 2, and a broken
  `docker.list` left on disk breaks every later `apt-get update` on the host.
  Both are now best-effort (`add_docker_repo()`), and the list file is
  removed again on failure. The Incus repository got the same treatment.

- **Docker: `docker-compose-v2` preferred, Buildx installed, daemon
  verified.** Compose v2 is now taken from the distribution where it exists
  (Ubuntu 24.04+/Debian 13+) before reaching for Docker's third-party
  repository; `docker-buildx-plugin`/`docker-buildx` is installed because
  `docker compose build` delegates to it; `docker.service` is `enable --now`'d
  and Step 4 checks `docker info`, so a CLI with no engine no longer reads as
  a healthy install. Verified: `docker-compose-plugin` has no `Depends:` that
  would displace `docker.io`, and it installs
  `/usr/libexec/docker/cli-plugins/docker-compose`, which the `docker.io` CLI
  (>= 20.10) finds.

- **Incus: the daemon init could be skipped on a slow host.**
  `init_incus_daemon` read `systemctl is-active incus` once, immediately after
  `enable --now`; a unit still starting made it return early and the first
  `vm/create` then failed with "No storage pool found". It now waits up to
  10 s for the unit to become active.

- **The installer is English only, end to end.** Its own messages already
  were, but apt/dpkg warnings, `systemctl` errors and — visibly — the whiptail
  buttons (`Oui`/`Non` on a French host) answered in the system language.
  `install.sh` now pins `LC_ALL=C`/`LANG=C` at the top, which also keeps the
  `apt-cache policy` probe deterministic (the localized `Candidat :` label used
  to make every availability check fail). Verified live: with
  `LANG=fr_FR.UTF-8`, whiptail renders `Yes`/`No`.

- **Five packages HomeVault actually executes were never installed.** Found by
  auditing every `Command::new("…")` and `resolve_bin("…")` call against the
  dependency inventory:
  - **`hdparm`** — the Power app's per-disk spindown (`src/power.rs`). The page
    reported "hdparm not available" and the setting never applied on a fresh
    install.
  - **`wpasupplicant`** (`wpa_cli`) — the Network app's Wi-Fi scan/join/status
    (`src/network.rs`).
  - **`xfsprogs`** (`mkfs.xfs`) — the RAID app formats ext4 **or xfs**
    (`src/disk.rs::format_device`); creating an XFS array failed at the mkfs
    step.
  - **`exfatprogs`/`exfat-utils`** (`mkfs.exfat`) — Explorer's "format as
    exfat". Only `exfat-fuse` was installed, which does **not** ship
    `mkfs.exfat` on Debian 12+/Ubuntu 23.04+, so the action was dead there.
  - **`avahi-utils`** (`avahi-browse`) — the mDNS half of Explorer's network
    discovery; `avahi-daemon` alone only publishes HomeVault.
  `samba-common-bin` (`smbtree`, the SMB half of the same list) is now
  requested explicitly instead of relying on Samba's recommends, and the
  Step 4 verification checks all six tools. `procps`, `debianutils` and
  `e2fsprogs` joined the required group so a trimmed image fails loudly.

- **`curl … | sudo bash` aborted the installation at the HTTPS question.** With
  the script on stdin, `read -r -p "Enable HTTPS…"` consumed the *next line of
  the script* instead of the keyboard, so bash died on
  `syntax error near unexpected token 'fi'` — after the binary had been
  extracted to `/opt/homevault` and the unit written, but **before**
  `daemon-reload`/`enable`/`restart` and the whole verification step. The
  install looked half-done and no service was running. Every prompt now reads
  fd 3, bound to `/dev/tty` when one exists, and `--yes` takes the documented
  default instead. Locked by `test_install_sh_never_reads_prompts_from_stdin`
  and `test_install_sh_binds_a_terminal_for_prompts` in `src/dependencies.rs`.

### Added

- **A dependency-completeness audit that runs in CI.**
  `dependencies.rs::tests::test_every_commanded_binary_has_an_install_source`
  walks `src/`, collects every binary passed to `Command::new("…")` and
  `resolve_bin("…")`, and fails on any name that is neither probed by the
  inventory nor listed in `BASE_BINARIES` (Priority: required packages) or
  `BINARIES_WITH_DEDICATED_CHECK` (module predicates, external GPU drivers).
  The inventory then pins every package to `install.sh` through the existing
  cross-check. Proven to bite: renaming the `hdparm` probe fails the test with
  `hdparm` in the report. Two more guards cover the English-only rule
  (`test_install_sh_is_english_only`, `…_pins_an_english_locale`).

- **The installer runs by CLI *or* by double-click, as one file.** Opening
  `installer/install.sh` from a file manager executes it with no terminal and
  as the logged-in user; the new entry bootstrap reopens it inside a terminal
  emulator and then re-execs through `sudo`, forwarding the documented
  `HV_RELEASE_*` knobs explicitly (never `sudo -E`, which default sudoers
  rejects). `HV_INSTALLER_STAGE` prevents the two re-execs from looping, and
  `--package`/`--dry-run`/`--help` are excluded from elevation entirely.

- **`whiptail` interface, with a plain-text fallback.** The interactive runs
  now use dialog boxes and a progress bar per step (Debian priority
  `important`, so already present on every supported base — it is never
  installed by the script). In TUI mode the engine's output goes to
  `/var/log/homevault-install.log` only and whiptail draws on its own fd, so
  the screens are not painted over. New flags: `--tui`, `--cli`,
  `--yes`/`-y`/`--non-interactive`, `--help`; `HV_MODE`/`HV_YES` do the same
  from the environment. The final "open the browser now?" prompt now follows
  the HTTPS choice instead of always opening plain HTTP.

- **`HV_RELEASE_TOKEN`, and a clear message when the release is private.** The
  installer downloads `homevault-<arch>.tar.gz` from the GitHub release, but a
  **private** repository answers `404` (not `401`) to an anonymous request, so
  the failure looked like a typo in the URL. The download now sends
  `Authorization: Bearer` when `HV_RELEASE_TOKEN` is set, and on a `github.com`
  URL the error explains the private-repository case and prints the token form.
  README and `docs/ARCHITECTURE.md` document it.

- **The AI model card now proposes five models, ranked for the machine it
  runs on.** `GET /chat/models/recommend` returned the best three of an
  eight-entry catalog with a scoring that could put a model too large for the
  host first. It now ranks the whole catalog with `fit_for()` — GPU fit
  (whole model in VRAM) first, then the largest model that fits in system RAM,
  then progressively smaller/faster ones — and returns exactly five
  (`RECOMMENDATION_COUNT`). When fewer than five fit, the list is completed
  with the next smallest, flagged `fits: false` and shown with a "needs N GB"
  chip; the "best choice" badge is only ever painted on a model that fits. The
  catalog was rebuilt on names that actually exist in the public Ollama
  library (`qwen3:0.6b` … `qwen3:8b`, `gemma3:1b`/`4b`, `llama3.2:1b`/`3b`,
  `qwen3:1.7b`/`4b`, `phi4-mini`, `mistral:7b`, `llama3.1:8b`,
  `deepseek-r1:1.5b`) — the old list carried `qwen3.5:*`, which does not
  resolve and would have failed at `ollama pull` in front of the operator.
  Locked by five new unit tests and E2E 9.5/9.6.
- **Ollama re-evaluated as the local runtime, and its one gap handled.** It
  stays the choice (single Go binary, model registry, streaming,
  `/v1`-compatible surface, automatic CPU/GPU split) against llama.cpp,
  LocalAI and vLLM — rationale in `docs/guides/chat.md`. It publishes no
  32-bit ARM build, so `install_ollama()` now skips `armv7l`/`armv6l` with an
  explicit message instead of leaving a daemon that never answers (asserted
  in `dependencies.rs`).
- **29 new bundled wallpapers (15 → 44).** `hv-16.webp` … `hv-44.webp` plus
  their `-thumb` previews, encoded from the Unsplash originals (full image long
  edge capped at 2560 px, WebP q82; thumb 640 px wide), and listed in
  `BUNDLED_WALLPAPERS`. Nothing else changes: still a pick-only grid in
  Settings → Appearance, no upload, no API. Thumbnails now span 3:2, 4:3 and
  16:9, which the existing `aspect-[8/5]` + `object-cover` tile already
  normalises. See `docs/guides/wallpapers.md`.

### Fixed

- **The topbar's network, notification and power popovers are on the theme
  ladder, and clicking inside the notification panel no longer closes it.**
  The three buttons themselves were already one shape (56px circle, glyph at
  half), but they painted a bespoke inset focus outline instead of
  `--hv-focus-ring`, the network popover used one-off literals (84px rows,
  52px tiles, a 48px copy button) with a **22px** IP value — the page-title
  step for a row value — and the notification panel titled itself at 17px (the
  *section* step) with 12px form text. All three now follow the shared scale:
  ~60px rows with 40px tiles and a 44px copy button, 15px row values and panel
  titles, 13px form text, and the shared focus ring on every control.
  Separately, the notification panel's root was the only topbar popover without
  a `stopPropagation` guard, so a click on a severity chip, the search field or
  mark-as-read bubbled to the shell's global click handler and dismissed the
  panel (E2E 16.2 could never pass). The power confirmation sheet got the same
  pass: 18px glyphs, 44px buttons and a focus ring on its close button and
  footer actions, which had **none** (WCAG 2.4.7).

- **E2E `desktop.spec.ts` 16.4 mocks the health call it actually makes.** The
  route glob was `**/api/health/**` while the desktop calls `/api/health` (no
  trailing segment), so the 426 never fired and the reload-banner assertion
  could not pass.

- **The account menu now sits on the theme's type and space ladder.** The
  topbar user popover ran on a one-off scale that belonged to no token step:
  a 22px name (the *page-title* size), 17px menu rows 73px tall, 25px glyphs,
  a 76px avatar, 24px padding and a full 420px width — a settings sheet
  hanging under a 52px pill. It now uses the same steps as every other topbar
  popover: name `--hv-text-md` (15px), role `--hv-text-sm` (12px), rows
  `--hv-text-base` (13px) at **44px** (`--hv-touch-target-min`), 18px glyphs,
  a 40px avatar (rendered at 72px, CSS-scaled so it stays crisp on HiDPI),
  16px header padding and a 320px panel (a menu, not a data panel). Rows are
  inset 8px so their hover wash is a rounded control inside the card, and they
  finally carry the shared `--hv-focus-ring` on `:focus-visible` — keyboard
  focus in this menu was previously **invisible** (WCAG 2.4.7). E2E 4.1–4.4
  (shared card chrome across the three popovers) stay green.

- **The window frame footer's icons no longer read a size larger than the
  header's.** The shared 36px strip shipped with 16px help / resize-grip
  glyphs next to the min/max/close trio's 14px ones, so the two rows of
  window chrome disagreed — and because the footer's assets are full-bleed
  circles while the trio's are inset, its 13.75px of ink outgrew the trio's
  8–12px. Both now clamp to one new token, `--hv-winctrl-glyph` (14px): the
  footer buttons stay 28px, only the glyph box shrinks, and their inline SVGs
  drop the `width`/`height` attributes so the CSS owns the box — the rule the
  header's app icon tile already followed. E2E `14.13` pins the parity.

- **The launcher's search bar no longer carries a blue ring.** The brand focus
  ring was on screen permanently — the dialog's focus trap lands on the query
  field the moment the launcher opens — and it was painted twice, because
  `base.css`'s global `input:focus` rule outranks the field's own class and
  stamped a second blue ring inside the capsule. The bar now takes a 2px slate
  rim drawn *inside* the capsule (`inset`, 7.1:1 on the white surface, no
  second hue) and the field paints no ring of its own. Keyboard focus stays
  visible: the rim appears only while the field holds focus. E2E `5.13` pins
  it (no brand blue, no field shadow, rim inset and ≥3:1, rim gone on Tab).

### Changed

- **Settings lost both icon-tile header tiers — titles and descriptions only.**
  The content header (the active category's title) and every `SettingsCard`
  header (TLS, Antivirus, Anti-Ransomware, Firewall, AI Assistant, UPS, SNMP)
  carried a 36px tinted icon tile beside the copy. The category glyph already
  sits on the active rail item, so the tile only repeated it while costing the
  panel 36px of height per header. Both headers are now two lines — title over
  description — with `px-6 py-2.5` / `py-2.5` instead of `pt-5 pb-4` / `py-4`,
  and `SettingsCard` drops its `icon`, `iconVariant` and `accent` props, which
  only ever coloured that glyph. Measured: content header **87.5px → 57px**,
  card header **83.5px → 57px**. Getting there also uncovered a silent 16px:
  `base.css` declares an unlayered `p { margin-bottom: var(--hv-space-4) }`, so
  the `m-0` both subtitles carried never applied — the new header measured
  73px until it took `mb-0!`. E2E 23.3 now pins the new shape (two children,
  no icon, header ≤ 64px) instead of the 36px tile.

- **One round icon tile on the whole Settings screen.** The screen drew three
  different tiles for one vocabulary: the rail category tile and the 11 Widgets
  rows were 28px squares with a 12px corner, while the 3 Privacy rows were 36px
  tiles with an 18px corner and a 20px glyph. Every decorative tile is now a
  **circle at `--hv-settings-icon-tile` (28px)** holding a 16px glyph, built
  from `settingsIconTile` in `vocabulary.ts` (the rail's comes from
  `WindowSidebarItem`'s rich CSS off the same token) — the same circle as the
  dock tiles, the topbar buttons and the window-header icon. The two icon-only
  buttons on the screen (the AI refresh control, the firewall confirmation
  dialog's close) are circles too, and three glyphs that sat off the icon ladder
  went back onto it: `14` in `PathPickerField`, `15` and `18` in the MCP card.
  E2E 23.12 walks all eight categories and fails on a tile of another size or
  shape, or on any glyph that is not 16 or 20.

- **Every wallpaper thumbnail is the same size.** The grid let each image pick
  its own height, and the bundled thumbs are not all one shape — `hv-09` and
  `hv-13` ship 640×480 while the other thirteen are 640×427 — so those two
  rendered ~13px taller than their neighbours and the rows came out ragged.
  `.settings-wallpaper-thumb` now takes a fixed `aspect-[8/5]` with
  `object-cover`, so all 15 tiles share one box whatever the asset's ratio;
  `object-cover` crops instead of distorting. `h-auto` stays deliberately: the
  `height="200"` attribute is a presentational hint that would otherwise
  defeat `aspect-ratio`. E2E 4.7 additionally requires every tile to measure
  the same width and height.

- **A fresh install opens with every desktop widget turned off.** All eleven
  monitors in `monitorStore.ts::defaultPositions()` now default to
  `*Visible: false` — the desktop is clean until the operator picks widgets
  from the Widgets app or Settings → Widgets. Positions are still seeded, so
  a widget switched on lands in its intended slot, and an account with a saved
  layout keeps its own flags (the stored value is read first). **Reset to
  Default** in the Widgets app now restores that same shipped default
  (everything off) instead of turning everything on. E2E `widgets.spec.ts`
  3.5 follows, and 2.2 enables the Resource monitor before measuring the
  panel geometry.

- **Adding a web shortcut moved off the desktop and into the launcher.** The
  "add shortcut" control used to sit at the bottom of the desktop's shortcut
  column, so a desktop that only exists to *show* what the user pinned also
  carried a configuration action — and the column was rendered even when the
  set was empty, leaving a lone "Add" tile floating on the wallpaper. The
  desktop now renders the pinned set and nothing else (no empty column), and
  the add tile became the launcher grid's **last cell**, after every app:
  same tile geometry, dashed ring instead of an app's identity gradient,
  `aria-label` from `shortcuts.add`. It is withheld while a search query or a
  category chip narrows the grid (a filtered grid answers "where is X?") and
  once `MAX_SHORTCUTS` is reached. The URL dialog moved to
  `components/desktop/ShortcutAddDialog.svelte`, owned by `Desktop.svelte`,
  so the launcher only fires a callback and closes. `shortcuts.open` /
  `shortcuts.remove` also stop shipping literal `{title}` to screen readers:
  the placeholder syntax is `%title%`, which is what the translator
  interpolates. E2E `shortcuts.spec.ts` re-targets the launcher tile (8
  tests; `1.2` pins "last cell, after every app") and `desktop.spec.ts`
  `5.14` excludes that tile from its A→Z run.

- **The external-agent (MCP) gateway is configured in Settings, not in the AI
  app.** The admin panel that used to slide in from the right of the Chat
  window (enable switch, permission editors, token mint/revoke) is now the
  `McpCard` of Settings → AI Assistant, next to the model card — the gateway
  is a server-wide admin configuration, so it no longer lives inside a
  conversation view. The card is admin-only and self-gating: it probes
  `GET /api/mcp/status` and renders nothing when that probe is rejected, so
  non-admins never see it. Its writes stay immediate (kill-switch, global
  permissions, one-time tokens) rather than joining the Settings draft — a
  deferred Save would leave the gateway open after the operator switched it
  off. `components/chat/McpPanel.svelte` is deleted; the Chat toolbar no
  longer carries the toggle. E2E moved to `tests/e2e/settings/mcp.spec.ts`
  (§15, 7 tests, incl. one that pins the Chat app free of the panel).

- **The desktop power menu now wears the dock's shape language.** The three
  actions behind the topbar Power button (Shut Down / Restart / Log Out) were
  three rounded cards; they are now three **circular gradient tiles with the
  label underneath**, exactly like a dock tile: `--hv-dock-item-size` diameter
  (56px desktop, 48px on the phone — still above the 44px target), glyph at
  half the tile, one hue per action (red / blue / slate), and the dock's own
  hover lift + `brightness(1.05)`. The button around the tile paints nothing
  and keeps the one control corner, so the focus ring and E2E `14.12`'s
  corner contract are unchanged. New E2E `7.5` pins the circle, the
  token-parity with the dock tile, the three distinct gradients, the
  label-under-tile anatomy and the touch target.

- **The launcher lists its apps A→Z, and its veil now hides the desktop.**
  Two pieces of user feedback on the same screen. (a) The tile grid was sorted
  by a hand-kept `CAT_ORDER` and then alphabetically inside each category, so
  the first tile was "System" and there was no alphabetical entry point into
  ~25 apps; the grid is now one A→Z list (`Intl.Collator` on the UI language,
  accent/case-insensitive, numeric), with the chip filter applied on top, and
  the category chips are sorted by their *localised* label so the row reads in
  the same order as the tiles under it. E2E `5.14` pins both, before and after
  a filter. (b) `--hv-launcher-scrim` `rgba(15,23,42,0.72)` → **`0.86`** and
  `--hv-launcher-blur` `2px` → **`6px`**: the desktop's icons and wallpaper
  stayed legible behind the launcher, which read as a translucent panel over
  the desktop instead of an app screen. Contrast only improves — the
  worst-case tile label (tile → veil → white paper) goes 5.5:1 → **8.7:1**.

- **Both desktop pills are 72px tall, and their controls grew with them.**
  `--hv-chrome-dock-h` 64px → **72px** and `--hv-chrome-topbar-h` 62px →
  **72px**, each on its own token (the dock and the topbar had been 84/62,
  then 64/62; above phone width they now match). Everything inside follows the
  reference dock's 0.775 ratio instead of a pixel count: dock tiles
  `--hv-dock-item-size` 50px → **56px** (glyph 25px → **28px**), topbar
  buttons `--hv-chrome-topbar-btn-size` 48px → **56px** (glyph 24px →
  **28px**), identity avatar 32px → **36px** (half the pill), dock count badge
  20px → **22px** (40% of the tile), dock group divider 32px → 36px, topbar
  divider now `calc(btn / 2)` = 28px. The dock's vertical padding stays
  derived — `calc((var(--hv-chrome-dock-h) - var(--hv-dock-item-size)) / 2)`
  = 8px — so the tile still fills the bar exactly; horizontal padding and the
  10px pitch are unchanged (the pitch is floored by the hover lift, now 5.04px
  a side). Phone keeps its compact pair: **56px** topbar with 44px buttons and
  a **52px** dock with 44px tiles, the only width where the dock is shorter
  than the topbar. `snapUtils.ts` moves with it (`topbarHeight: 80`,
  `dockHeight: 72`), and E2E `8.3` now pins 72/72 above 480px.

- **The dock is 64px tall on desktop/tablet (was 84px), 52px on phone.**
  `--hv-chrome-dock-h` drops from the mockup's authored 84px, and the tile
  follows the reference dock's 0.775 ratio down with it: `--hv-dock-item-size`
  64px → **50px**, glyph 32px → **25px** (now `calc(tile / 2)`), count badge
  26px → **20px** (the same 40%-of-tile proportion, matching the phone's own
  value), group divider 40px → 32px (`0.64 × tile`). The bar's vertical padding
  is now **derived** in CSS — `calc((var(--hv-chrome-dock-h) -
  var(--hv-dock-item-size)) / 2)` = 7px — so a future height change can never
  leave dead space around the icons; horizontal padding and pitch stay at 10px
  (the pitch is floored by the hover lift: 4.5px a side, so ~9px minimum).
  Phone goes 56px → 52px = the 44px touch floor (its tile size, unchanged) +
  4px a side, which makes the phone the one width where the dock is shorter
  than the topbar pill. `snapUtils.ts` (`dockHeight: 64`) and the desktop E2E
  lockstep test moved with it.

- **Every app window now ends in a shared frame footer.** `WindowFrame`
  renders one 36px chrome strip under the page: a help button on the far left
  (opens the in-product assistant) and the window's resize grip on the far
  right. The grip replaces the invisible 18px SE corner hotspot and uses the
  `resize-circle-horizontal` glyph rotated 45° clockwise, so it reads as the
  bottom-right diagonal it pulls; it is a real button (pointer drag + arrow
  keys, 16px a press, 64px with Shift) and is omitted while the window is
  maximized. The seven edge strips are unchanged. New i18n keys
  `desktop.window_help` / `desktop.window_resize` in all four locales. See
  `docs/guides/desktop.md` → *Frame footer*.

- **Design tokens now follow `design-tokens.md` for everything except colour.**
  The Apple-style spec at the repo root is adopted as the reference for the
  non-colour craft, translated onto the `--hv-*` vocabulary in
  `frontend/src/lib/styles/theme/tokens.css`: a layout rhythm
  (`--hv-section-gap-*`, `--hv-component-gap-*`, `--hv-card-padding-*`,
  `--hv-content-max-width*`), a 8/12/18 corner ladder
  (`--hv-radius-badge` / `-default` / `-2xl`, plus `--hv-radius-pill` for
  tags), multi-layer contact + ambient elevation (`--hv-shadow-*`), the 400–900
  weight scale, 1.1 title / 1.53 body leading, -0.015em title tracking and the
  734/1068/1440 layout breakpoints. `.hv-card` takes the 32px card padding and
  the resting card shadow; `.hv-badge` takes the 8px badge corner. Its **colour
  palette is deliberately not adopted** — one accent (`#1E62AE`) and the
  blue-grey ink ramp stay the only colour authority — and form buttons keep the
  control corner rather than the reference's pill (one documented deviation).
  See `docs/guides/design-system.md` §2.2 and §3.2.

### Added

- **Scheduled Tasks** (new app, migration `043_scheduled_tasks.sql`): one
  generic cron engine replacing per-feature schedulers for admin-defined
  recurring jobs — single-executable commands (no shell, absolute path,
  30-minute timeout), pool snapshots, S.M.A.R.T. self-tests and host power
  actions. Payloads are validated at creation *and* at run time
  (`TaskAction::from_parts`); every execution is recorded in a run history
  with an output tail. See `docs/guides/scheduled-tasks.md`.
- **Metrics** (new app, migration `044_metrics_history.sql`): a 60 s
  collector (CPU deltas, memory, load, network throughput, max thermal-zone
  temperature) with 8-day retention, bucketed 1 h / 24 h / 7 d SVG charts,
  gaps rendered as gaps. See `docs/guides/metrics.md`.
- **Host power management + disk spindown** (Settings → System, migration
  `045_disk_power.sql`): CPU governor (validated against the kernel list),
  host reboot/shutdown/suspend, and per-disk hdparm APM + idle timeout that
  are re-applied at every startup. See `docs/guides/power.md`.
- **Per-event notification toggles + webhook sink formats**: every engine
  notification carries a stable event id (`backup.jobs`, `app.updates`,
  `health.<check>`); admins mute events individually
  (`GET|PUT /api/notifications/events/config`, absent = enabled) and choose
  the webhook payload shape (`generic` | Slack | Discord). See
  `docs/guides/notifications.md`.
- **Snapshot schedules with GFS retention** (Storage → new "Snapshots" tab,
  migration `046_snapshot_schedules.sql`): cron-driven `{prefix}-<timestamp>`
  snapshots per pool with grandfather-father-son pruning (newest N
  hourly/daily/weekly/monthly/yearly generations); pruning only ever deletes
  snapshots carrying the schedule's own prefix — manual and backup snapshots
  are untouchable. See `docs/guides/snapshots.md`.
- Fixed a latent `api.ts` bug where `ttl: 0` is falsy and silently fell back
  to the 5 s default TTL (the topbar notification list, What's New, VPN peer
  detail and SMART capabilities stayed stale for up to 5 s after a mutation;
  now `{ cache: false }`).

- Persistent notification center: the bell panel now keeps the full history
  with severity filters, an unread-only toggle and text search, and the
  "What's New" release-highlights modal shown once per version per account
  (`GET|POST /api/whats-new[/seen]`, migration `042_whats_new.sql`,
  `src/release_notes.rs`). See `docs/guides/notifications.md`.
- Storage health: a per-virtual-disk scrub-status endpoint + inline UI
  (`GET /api/storage/virtual-disks/{name}/scrub`) and foreign-pool import for
  ZFS/Btrfs pools discovered on attached disks (`GET /api/storage/importable-pools`,
  `POST /api/storage/import-pool`). See `docs/guides/storage.md`.
- Hardened app-proxy auth: proxied apps no longer receive the API session.
  The proxy gate accepts only an app-proxy-scoped `hv_app_session` token minted
  by the server-chosen `GET /app-auth` entry, and both HomeVault cookies are
  stripped before forwarding upstream. See `docs/guides/docker.md`.
- Versioned client/server contract: `GET /api/meta` + the
  `X-HomeVault-Contract` header, with a 426 + reload banner on a mismatch
  (`src/api_contract.rs`, `src/middleware/contract.rs`). See
  `docs/ARCHITECTURE.md`.
- Development tooling: `scripts/dev-instance.sh` (multi-instance `.local`
  dev via `HV_MDNS_HOSTNAME`), a real-VM Playwright suite
  (`playwright.vm.config.ts`, `tests/vm/*.vm.test.ts`), and
  `docs/guides/development.md`.

- External AI-agent server (MCP, Streamable HTTP): `POST /mcp`
  (`initialize`, `tools/list`, `tools/call`) with opaque `hv_mcp_…` bearer
  tokens, one master kill-switch and one global permission set shared by all
  tokens (apps / App Store / files / VMs / knowledge base / reboot — deny by
  default), managed from an admin-only "External agents" panel in the Chat app
  (`GET|PUT /api/mcp/status`, `PUT /api/mcp/permissions`, `POST|DELETE
  /api/mcp/tokens`). The `knowledge.search` tool exposes the internal RAG base
  to agents. See `docs/guides/mcp.md`.
- App catalog phase B (feature parity on the same `homevault-app.toml` shape):
  dependencies with `implements` alternatives + install-review endpoint,
  `[[environment]]` install settings rendered as `{{NAME}}`, `arch` +
  `min_homevault_version` compatibility (listed and enforced), per-app compose
  logs, and a sequential `update-all` queue. Install/uninstall share their core
  with the MCP `apps.*` tools. See `docs/guides/docker.md`.

- GitHub community health files: `LICENSE` (MIT), `SECURITY.md`,
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, issue/PR templates, Dependabot,
  `CODEOWNERS`, `.editorconfig`, `.gitattributes`.
- SPDX metadata (`license`, `repository`, `homepage`) in `Cargo.toml`,
  `frontend/package.json`, and root `package.json`.

### Added

- **Automatic snapshots of HomeVault's own database** (`src/db_backup.rs`,
  migration `048_app_meta.sql`). The database runs in WAL mode, so copying
  `homevault.db` on its own yields a file with **no tables at all** (measured:
  a 4 KB `.db` next to a 1.8 MB `-wal`) — the obvious manual backup silently
  produced nothing restorable. Snapshots are written with `VACUUM INTO`, which
  produces a single self-contained file: at startup when due, daily, before any
  migration is applied, and on demand. Settings → System → *Database snapshots*
  shows the last run, the schedule and the files on disk.
- **Liveness and readiness probes** — `GET /api/health/live` (public) and
  `GET /api/health/ready` (public, 503 when the database cannot be queried or
  was found damaged at boot). `GET /api/health` previously answered **HTTP 200**
  in both of those cases, so no monitor could detect an outage from it.

### Security

- **CSP no longer allows `'unsafe-inline'` scripts.** `script-src` lists a
  per-response nonce instead, and the single inline boot script in `index.html`
  carries it. With `'unsafe-inline'` plus the session token in `localStorage`,
  any injected script could read the token and exfiltrate a full session.
  `style-src` keeps `'unsafe-inline'` (Svelte scoped styles and `style=""`
  attributes cannot be nonce'd) — documented debt in `guides/security.md`.
- **The installer can roll back and uninstall.** `--rollback` swaps in
  `/opt/homevault/homevault.previous` (kept by every upgrade); `--uninstall`
  removes the app but **keeps** the data, and `--purge` deletes it behind a
  typed `PURGE` confirmation; `--dry-run` prints every action and changes
  nothing (and works without root). A failed release no longer leaves an
  appliance with no way back.
- **HTTPS now actually protects the login.** Enabling TLS left the plain-HTTP
  listener serving the wizard and the login form — admin password in clear — and
  `redirect` defaulted to **off**, so nothing changed in practice. It now
  defaults to **on** (opt out with `TLS_REDIRECT=0`), a legacy `data/tls.json`
  picks up the safe default via `serde(default)`, and the installer's HTTPS
  question defaults to **yes**. The session cookie gains `Secure` when the
  request arrived over TLS, and the wizard + login screen show a
  `role="status"` warning when served over plain HTTP on a non-loopback host.
- **Removed the dead `fail2ban` middleware.** `src/middleware/fail2ban.rs`
  defined an in-memory IP ban list that no route or handler ever referenced — it
  read like an active protection while doing nothing. Brute-force defence is the
  per-account lockout and the per-IP throttle (see below).
- **The login lockout is temporary again.** Five wrong passwords used to set
  `users.locked` and never clear it, so on a single-admin appliance a typo storm
  bricked the only account — the message said "temporarily locked" while the
  only ways back in were the offline recovery code or editing the database. A
  lock now records `locked_until` (migration `047`): an automatic lock lifts
  itself once the timestamp has passed and a successful login clears it too,
  while a deliberate admin lock (no `locked_until`) stays permanent. The
  migration repairs accounts already bricked by the counter.
- **Authentication is throttled per client IP.** The only counter was per
  account, so password spraying — one password tried across many usernames —
  was unlimited. `AuthService::check_ip_rate_limit` now runs before the account
  lookup on both `/auth/login` and `/auth/2fa/verify`: 30 attempts/minute and
  120/hour per IP, returning `429`. The handler honours that status instead of
  collapsing it into a 400.
- **The client IP is the TCP peer, not `X-Forwarded-For`.** Trusting the header
  outright let anyone forge the address recorded in the login history — and,
  now that the throttle is keyed by IP, sidestep it by rotating a header value.
  The headers are honoured only from a loopback peer (a reverse proxy on this
  host) or an address listed in `HOMEVAULT_TRUSTED_PROXIES`.
- **Login accessibility:** the inline error is now a `role="alert"` /
  `aria-live="assertive"` region, the credential fields carry `aria-invalid` +
  `aria-describedby`, and closing the error modal returns focus to the form
  instead of dropping it on `<body>`. The locked-account message points at the
  recovery-code path rather than "contact support".
- **The first-run bootstrap endpoints can no longer be driven from a foreign
  web page.** The CORS layer is permissive (`allow_origin(Any)`), so the
  local-network guard alone left a hole: the operator's browser *is* on the LAN
  and the mDNS name is predictable, so any site they visited could
  `POST /api/auth/setup` and claim the first admin. `middleware::setup_guard`
  now also refuses a foreign `Origin`, `Origin: null` and any `Sec-Fetch-Site`
  other than `same-origin`/`none` (403), and rate-limits the guarded routes to
  20 requests/min per peer (429). `HOMEVAULT_SETUP_ALLOW_REMOTE=1` no longer
  bypasses these two checks.
- **The setup lock is self-healing.** `.setup_lock` records `pid` +
  `created_at`; a dead PID or an age over 15 minutes makes it reclaimable
  (`mark_setup_in_progress` removes and re-creates it under `O_EXCL`), and
  `cleanup_stale_setup_lock()` runs at startup. A crash, an OOM kill or a power
  cut during setup used to dead-end the wizard permanently — `is_first_run`
  stayed true while every submission answered "Setup is already in progress".
- **Password policy raised to 12 characters, with a compromised-password
  check.** `SetupService::validate_password` is now the single policy (the
  wizard, admin-created users, password changes and recovery-code resets all
  delegate to it): 12–128 characters, mixed case + digit, not on the
  obvious/breached list (`Password1234`, `Homevault2026`, `qwerty…`, …) and no
  character repeated more than four times in a row. The wizard's live checklist
  lists all six rules and gates its "Next" button on them.
- **The recovery-code screen is blocking.** A failed generation now offers a
  retry instead of a "continue": the code is the only offline way back into the
  account (there is no email reset), so the wizard no longer lets an admin walk
  away without one.

### Removed

- **Setup Wizard: the storage, RAID, restore *and language* steps are gone** —
  the wizard is down to 4 steps (welcome, account, password, verify). RAID is
  configured from its own app, the appliance is installed by script on an
  existing OS, a backup is restored from the Backup app, and the interface
  language is now **detected from the OS/browser locale** (English when
  unsupported, changeable in Settings) instead of being asked during
  onboarding. Removed with them: `GET /auth/setup/storage-options`, the
  frontend `getSetupStorageOptions` service method, the
  storage/RAID/restore/language step markup in `SetupWizard.svelte`, the
  `setup.storage.*` / `setup.raid.*` / `setup.restore.*` / `setup.language.*`
  i18n keys in all four locales, and the matching integration test.
  `setup_progress.data` now carries nothing.

### Added

- **OS locale detection for the interface language**: `GET /auth/setup-status`
  reports `system_language` (`SetupService::detect_system_language()` reads
  `LC_ALL` → `LC_MESSAGES` → `LANG` → `LANGUAGE`, then `/etc/default/locale`),
  and `languageStore.applySystemLanguage()` uses it when the user made no
  explicit choice and the browser locale is not one HomeVault ships. Settings
  remains the only place a preference is stored.

### Fixed

- **An older HomeVault binary silently ran against a newer database.** Migrations
  are append-only and never reverted, so a downgrade started happily and then
  failed on the first query expecting a column it does not know about. Startup
  now compares `app_meta.schema_version` (migration `048`) with the highest
  migration embedded in the binary and **refuses to start** with a message that
  says what to reinstall — and `src/bin/server.rs` propagates the failure so the
  process exits non-zero (`systemd Restart=on-failure` can see it).
- **A damaged database was only discovered at the first query.** Startup runs
  `PRAGMA quick_check(1)` and surfaces a failure through `/api/health/ready`
  (`503 degraded`) instead of waiting for random queries to blow up.

- **The AI assistant could not answer about 25 of HomeVault's own features.**
  The RAG knowledge base indexed 20 feature documents while `docs/guides/` had
  grown to 46 files, so a question about Syncthing, the downloader,
  notifications, the desktop shell, LUKS encryption, RAID, the cache tier,
  widgets or a dozen other shipped features got "I don't know about that". The
  knowledge base now documents all 45 indexable guides — including the account
  page, login and recovery, the setup wizard, groups, antivirus,
  anti-ransomware, the firewall, LUKS encryption, the RAID wizard, snapshots,
  the cache tier, Syncthing, the downloader, the AI chat and its knowledge
  base, notifications, widgets, the desktop shell, the app launcher, themes,
  wallpapers, transfer quotas, mDNS, server control and avatars — and guide
  coverage is enforced by a test that enumerates `docs/guides/` instead of
  relying on the written rule.
- **Cloud Sync read and wrote a different rclone config than the one it parsed.**
  `GET`/`POST /rclone/config` and the add/delete-remote handlers hardcoded
  `/etc/rclone.conf`, while the remote list honoured the `RCLONE_CONFIG`
  override — so an admin whose config lives elsewhere saw a list and an editor
  describing two different files, and saving failed with `Permission denied`
  against the root-owned default. All four now go through a single
  `rclone_config_path()` resolver, which `parse_rclone_config()` uses too.
- **`JobType::Custom` could not be serialized, which silently broke progress
  reporting for every feature job.** `JobType` was internally tagged
  (`#[serde(tag = "type")]`) with a `Custom(String)` newtype variant, which
  serde cannot serialize at all. `handle_progress_sse` falls back to an empty
  payload on a serialization error, so a directory copy/move, Docker install,
  antivirus scan, RAG reindex, VM create, rescue, cloud sync or Ollama pull
  emitted an empty SSE `data:` line — the client's `JSON.parse` threw, no
  progress or completion callback fired, and the Explorer's directory
  copy/move reported failure for a job that had succeeded. `GET
  /api/progress/jobs` also panicked while any such job was tracked. Now uses
  serde **adjacent** tagging: built-in kinds keep their exact wire shape
  (`{"type":"backup"}`), feature jobs become
  `{"type":"custom","name":"explorer_copy"}`.
- **`GET /api/progress/{job_id}/status` returned a placeholder.** It answered
  a hardcoded `Running` / 0 % / `"Waiting..."` for any tracked id, so a job at
  87 %, a completed job and a failed job all read back as "running, 0 %". It
  now returns the job's stored last event — the same snapshot as the SSE
  stream and the activity feed — and 400s for an unknown or removed id.
- **Deleting a user now revokes the share permissions it held directly.**
  `handle_user_delete` previously only *logged* an intent to revoke: the
  `share_permissions` rows (keyed by grantee name, with no foreign key to
  `users`) and the real POSIX ACLs were both left in place, so recreating an
  account with the same username silently inherited the deleted user's access.
  The handler now calls `PermissionService::revoke_all_user_permissions`, which
  removes the filesystem ACLs (best-effort) and the database rows. Group grants
  are untouched — they belong to the remaining members. Regression guard:
  `tests/users_integration.rs` →
  `test_user_delete_revokes_its_direct_share_permissions_only`.

### Changed

- **One corner language across the shell, the apps and the gateway.** An
audit found the radius scale applied unevenly: the desktop widget/monitor
panels, the notification list rows and the floating path picker carried
literals off the appliance stops (28px power dialog, 20px notification
rows, 14/7/9px picker parts), a dead `--hv-radius-pill` token, an undefined
Tailwind `rounded-4xl` on the badge, and `calc(var(--radius) - 5px)` (1px)
inside the input group. Every corner now reads a stop from
`docs/guides/design-system.md` §3.2 — window `--hv-radius-2xl` (16px),
floating chrome panels `--hv-radius-4xl` (24px), cards/modals/pickers
`--hv-radius-xl`/`--hv-radius-card` (12px), controls/fields/buttons
`--hv-radius-control`/`--hv-radius-md` (8px), pills/dots
`--hv-radius-full`. The login and setup-wizard fields and buttons moved off
`rounded-lg` (10px, the card stop) onto the control stop so they match the
shared `Input`/`Button`. Guards: `desktop.spec.ts` 7.4 (power dialog ==
topbar popover = 24px), `login.spec.ts` 13.7 (setup card 16px / login field
8px). The `--hv-radius-4xl` documentation row was corrected (it never was
the login/setup card stop) and the stale `--hv-radius-sm` (2px) note in the
theme README was fixed.

- **The WebDAV server is now part of HomeVault (Rust), not a generated
  Python script + root systemd unit.** `src/webdav/` is an in-process axum
  listener built on `dav-server` (same hyper 1.x stack as the app proxy),
  managed by `WEBDAV` (`start`/`stop`/`reload`, mirrors `APP_PROXY`). It also
  fixes three real defects in the old implementation: Basic auth now actually
  verifies the HomeVault credentials (the Python `_check_auth` accepted any
  `Authorization: Basic` header), the URL's first segment is a **share name**
  confined to that share's root (the old server ignored `path_prefix` and
  served the process CWD as root), and `PROPFIND` is authenticated. The
  `share` grants of `resource_shares` are enforced per member on every
  operation, and `path_prefix` / `require_auth` / `require_ssl` / `locking` /
  `allowed_methods` are all honored (the default config is closed: port 8082 +
  auth on). An upgraded host disables and removes the legacy
  `homevault-webdav.service` + `homevault-webdav.py` at startup. See
  `docs/guides/shares.md`.

- **Static assets are now cached, validated and pre-compressed.** The
  `include_dir!`-embedded files used to be served as raw bytes with no
  `ETag` and no `Cache-Control` on `/wallpapers/*`, `/static/*` and the icon
  sprites, so a browser could never revalidate and re-downloaded several MB
  per visit. `spa.rs` now builds a `path → Asset` table once, sends an `ETag`
  (distinct per content coding), answers `If-None-Match` with `304`, and sets
  the cache lifetime per route — `immutable` for the content-hashed bundles,
  the wallpapers and `static/`; one day plus revalidation for the
  regenerated sprite; nothing for `index.html` / `sw.js`. The blanket
  `SetResponseHeaderLayer` for `/assets` and `/fonts` was replaced by this
  per-route policy. `frontend/scripts/precompress-dist.mjs` (npm `postbuild`)
  writes `.br` / `.gz` siblings that the server serves directly instead of
  re-compressing every response, and the `tower-http` `fs` + `set-header`
  features are no longer enabled.
- **The unified icon sprite is subset to what the frontend references**,
  dropping 7 750 glyphs to ~110 (3.6 MB → ~55 KB, and ~12 KB brotli), with
  `mapping-unified.ts` filtered to match (485 KB → 6 KB). Regeneration is now
  part of `npm run build` (`prebuild`); `HV_SPRITE_FULL=1 npm run
  icons:generate` restores the complete sprite.
- Release profile: `strip = true` and `panic = "abort"` (smaller binary; a
  panic now aborts the process instead of unwinding one task, so recovery
  relies on the systemd unit's `Restart=on-failure`).
- `installer/` is now a **single self-contained `install.sh`**. The systemd
  unit and the udiskie automount config are written inline; the sibling
  `homevault.service`, `setup-automount.sh`, `optional-packages.conf` and
  packaging `Makefile` are gone. The application binary is downloaded as a
  per-architecture asset (`homevault-<arch>.tar.gz`) from GitHub Releases
  (with optional SHA-256 verification), and `install.sh --package` builds
  those release assets from a checkout.
- **The login screen and the setup wizard now use the desktop's palette
  instead of a second identity of their own.** Both carried a page-scoped
  indigo/violet "gateway" palette — a second accent, a pastel gradient and a
  slate ink ramp one step lighter than the app. Every `--hv-login-*` /
  `--hv-wizard-*` value is now an alias of an Appliance token: the accent
  *is* `--hv-primary` (#1E62AE), the canvas *is* `--hv-desktop-bg` (declared
  once — `Desktop.svelte` no longer keeps its own copy of that gradient), the
  ink is `--hv-shell-text-*`, and the card borrows the desktop window's
  hairline and elevation. The wizard's emerald family (`#10B981` / `#22C55E` /
  `#16A34A` and its create gradient) failed WCAG 1.4.3 / 1.4.11 in every role
  it carried ink (2.28–3.77:1) and was replaced by the Appliance success ramp
  (`#1E7E34` / `#166428`); the amber recovery box is now white paper inside a
  `--hv-warning` border with an amber code chip, and the login screen uses one
  error red instead of two. `login.spec.ts` 13.6 asserts the aliases *and*
  every contrast floor of the gateway palette. See
  `docs/guides/design-system.md` §6.1/§6.2.

## [1.0.0] - 2026-10-10

### Added

- Rust (Axum) backend + Svelte 5 frontend NAS appliance: dashboard, users
  and groups, storage and RAID, shares (SMB/NFS/FTP/SFTP/WebDAV), file
  explorer with trash, restic backups, rclone cloud sync, network,
  antivirus and firewall, Docker with an app catalog, virtual machines
  (Incus), VPN (WireGuard/OpenVPN), Syncthing, aria2 downloader, local AI
  chat (Ollama) with a RAG knowledge base, widgets, health monitoring,
  system journal, transfer quotas, web terminal, and settings.
- SQLite migrations (`sqlx/`), installer (`installer/`), and reference
  documentation (`docs/`, `TESTING.md`, `ROADMAP.md`).

[Unreleased]: https://github.com/nicoleroux35/HomeVault/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/nicoleroux35/HomeVault/releases/tag/v1.0.0
