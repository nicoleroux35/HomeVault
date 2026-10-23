# Changelog

All notable changes to HomeVault are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versioning
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
