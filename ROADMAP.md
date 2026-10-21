# HomeVault Roadmap

Last updated: 2026-10-21 — current version: 1.0.0

## Current state

HomeVault today covers: dashboard, users/groups (a forced first-sign-in
password change on admin-chosen passwords, admin-side session audit/revoke,
per-account expiry horizons, per-member *and* per-group resource access, and
group rename/description — all 2026-10-20), storage
(disks/partitions/ZFS-Btrfs-mdadm virtual disks/LUKS2
encryption/SMART health), RAID (mdadm/ZFS/Btrfs creation), shares
(SMB/NFS/FTP/WebDAV/SFTP), file explorer, backups (restic), cloud sync
(rclone), network (interfaces, WiFi, diagnostics, **persisted interface configuration
applied, verified and rolled back in-session via netplan/ifupdown as of
2026-10-21**), security
(antivirus/anti-ransomware/firewall), web terminal, VPN (WireGuard/
OpenVPN), downloader (aria2), Syncthing, Docker + application catalog
(volumes, disk-usage reporting and prune, container inspect/exec/pause/
rename/in-place limits, live log streaming, image builds, private
registries, `daemon.json` editing, update-to-newest-image — all
2026-10-20),
virtual machines (Incus), AI assistant (Ollama + RAG, plus an MCP gateway
for external agents), health, system journal, transfer quotas, historical
metrics (1 h / 24 h / 7 d), a generic scheduled-task engine (commands,
snapshots, SMART tests, power actions), host power management with per-disk
spindown, snapshot schedules with GFS retention, a notification center with
per-event toggles and webhook sinks, and a suite of desktop widgets.

Every screen listed above has undergone a "real conditions" audit (real
backend, real session, real browser when possible) documented in
`TESTING.md` and `docs/guides/`. There are no frozen areas
(`CLAUDE.md` → "Design system governance"): every page remains editable,
as long as its tests stay green (Rust + Playwright E2E).

> **Note on this file's reliability:** the first version of this roadmap
> (2026-07-21, before the verification pass below) had been compiled by
> re-reading the text of `CLAUDE.md` without re-checking the real code. A
> second pass the same day showed that **6 of the 7 items listed as
> "not fixed" were in fact already resolved** in the code (fixes already
> present but never reflected in the text of `CLAUDE.md`). The
> corresponding corrections were added as dated addenda directly in
> `CLAUDE.md`. Lesson learned: always verify against the current code
> before trusting an audit text, even in this file — see "Process"
> below.

## Medium priority — incomplete features

None at the moment. Re-audited against the code on 2026-10-12 (not against the
text of `CLAUDE.md` — see "Process"); the three items that pass found are
fixed and listed under "Fixed on 2026-10-12" below.

## Ideas / backlog

- **Consolidate `Docker.svelte` onto `api.ts`.** It drives the whole Docker
  surface through its own `authedFetch` while `api.ts` exposes 6 Docker methods
  used by the desktop monitor. The two are **behaviourally identical today**
  (same session + CSRF headers, checked 2026-10-12 — see
  `docs/guides/docker.md`), so this is maintainability, not a bug. Deliberately
  deferred: it means ~50 new wrapper methods and ~100 rewritten call sites,
  with `docker.spec.ts` (58 tests) as the only net. If taken up, do it one tab
  at a time, E2E green between each.
- Keep auditing "under real conditions" (real backend, no mocks) every
  new page, following the methodology already applied to each guide in
  `docs/guides/` (results in `TESTING.md`).

**Fixed on 2026-10-21 — Users/Groups production-ready:**

Deleting an account did not delete the Linux account behind it, and demoting an
admin did not take its sudo away. Four holes, all closed by the new
`src/host_accounts.rs` (`HostAccountService`):

1. `DELETE /users/{u}` dropped the row only. A deleted admin kept a working
   Linux login *and* — admin status is derived from `homevault_admin` — a
   working HomeVault **admin** login that the next PAM sign-in recreated.
   `userdel` now runs **before** the row delete (retryable on failure); a
   PAM-mirrored row whose Linux user still exists is refused, since HomeVault
   did not create it.
2. Demotion cleared only `homevault_admin`, leaving `sudo`/`wheel`, and the
   session kept its `is_admin` snapshot (up to 30 days). All three groups are
   revoked **before** the role write, the demoted account is signed out, and
   `get_session` re-derives `is_admin` from the current role.
3. Locking mirrored nothing to the host (`usermod -L/-U` now does).
4. A password reset on a PAM-mirrored account did nothing (sentinel hash);
   `chpasswd` on stdin now follows it.

Guard rails: username gate before any command, system-account refusal
(`uid < 1000` / `>= 65534`), 20 s timeout on `tokio::process`, `userdel`
without `-r` (home removal stays the explicit `delete_home_folder` choice).

Groups: members must be existing HomeVault users (each one gets a Linux
account); blank names and >500 members refused; `DELETE /groups/{name}` purges
all four display-name references, `share_permissions` included; `GET
/users/{u}/groups` is admin-or-self; and every `GroupService` call goes through
`run_group_op` (`spawn_blocking`, 15 s) instead of blocking a Tokio worker.

Tests: `host_accounts` unit 14, `users_integration` 79, `groups_integration`
38, `groups.rs` unit 9, `users.spec.ts` +5.10b. The six real host effects are
`#[ignore]`d root-only tests (`tests/login_pam_integration.rs`), not run here —
no disposable host account. See `docs/guides/users.md` → *Host (Linux) account
lifecycle*, `docs/guides/groups.md` → *Membership validation and delete
cleanup*, and `CLAUDE.md` (2026-10-21 addendum).

**Fixed on 2026-10-21 — Rescue production-ready:**

The Rescue app is the one screen allowed to write to raw block devices, and its
"Cancel" was theatre: `DELETE /api/progress/{job_id}` dropped the job from
`PROGRESS_MANAGER` and answered "Job cancelled" while `sudo ddrescue` kept
writing to the destination disk. Nothing observed the request, and the UI had no
stop control at all — a clone of a failing 4 TB disk could only be aborted by
killing the server. `ProgressManager` now carries per-job cancellation hooks
(`register_cancel_hook` / `cancel_job` / `CancelOutcome`, emitting the
`ProgressState::Cancelled` that already existed and had never been used), and
`src/rescue/jobs.rs` runs each rescue job in its own process group so a cancel
SIGTERMs the group (escalating to `SIGKILL` after 10 s) — killing only the
`sudo` wrapper left the real tool running. A cancelled clone reports *cancelled*
and keeps its mapfile, so it can be resumed.

`GET /api/rescue/ddrescue/status/{logfile}` took the raw URL segment and read it
with the service's own privileges: an arbitrary-file read and a file-existence
oracle behind any admin session. It is now confined to `/tmp`, `/var/tmp` and
`paths::rescue_dir()`, must be an existing regular file with no symlink in the
final component, and is capped at 16 MiB. The same non-following and
component-wise checks (`/tmpfoo` is not inside `/tmp`) guard output paths.

A clone — irreversible — had no safety net: nothing checked that source and
destination were different devices, that the destination was not mounted or an
active md/LVM/dm-crypt member (sysfs `holders`, which catches a member disk with
no mountpoint of its own), or that it was at least as large as the source.
`check_clone_safety` now does all three, and the protected-device rule no longer
trusts a hardcoded `sda`/`nvme0n1`/`vda` list: the live root disk is resolved
through `findmnt /`, and device names are matched partition-aware (`sda1`
belongs to `sda`; `sdaa` does not).

Finally, options that lied. TestDisk's `deep_search`/`log_path`/`force` and
PhotoRec's `keep_broken`/`max_size` were accepted, shown as applied, and never
reached the tool; `mdadm`'s `action` went verbatim into `--action=`;
`file_types` were spliced into PhotoRec's comma-separated `/cmd` grammar, so
`"jpg,search"` injected a command. Un-honourable options are now a `400` naming
the field (and gone from the UI — TestDisk's advanced block went with them), and
the dead fields were dropped from the model rather than kept as switches that
change nothing. Every subprocess runs under a timeout, the default mapfile left
world-writable `/tmp` for the root-owned `rescue/` directory, and PhotoRec's
recovered tree is `chown -R`'d to the admin who asked for it.

Tests: `rescue` unit 8 → 44, `sse::progress` unit 20 → 26, `lib` 965 → 999,
`rescue_integration` 28 → 38, `progress_integration` 7 → 8, E2E
`rescue.spec.ts` 53 → 59. Live-host checks (a real clone, a real cancel, the
PhotoRec chown) are listed in `TESTING.md` → "Remaining to test as a priority".

**Fixed on 2026-10-21 — Shares production-ready:**

The Shares app was the one screen whose inputs still reached **root-owned**
config files unvalidated, so an admin session (or anything that can reach the
API) could rewrite system configuration: a share name with a newline forged an
extra `smb.conf` section (an `include =` directive is root code execution on
the next Samba reload), a `)` in an NFS option closed the client list and
fabricated `*(no_root_squash)`, a whitespace in a path shifted `/etc/exports`
and `/etc/fstab` columns, a multi-line FTP banner appended `anonymous_enable=
YES`, and the SFTP jail wrote a share path straight into `/etc/fstab`. The
pass adds one validation layer in `SharesService` (share name/comment/path/NFS
options, SMB global config, FTP config) that runs before *any* privileged
write, so every caller is covered — handlers answer `400` naming the field.
`/etc/exports` now has a single pure parser/generator pair: the enable/disable
toggle writes (or strips) a `# homevault-nfs-disabled:<name>` tag instead of
the previous wrong-shape `exportfs -o rw /<name>` / `exportfs -u /<name>`
no-op that silently reverted on reboot, and delete/update rewrite exactly one
line — two exports sharing a path basename used to delete each other, they are
now an explicit `400` listing the colliding paths. Finally the whole module
left the async runtime: `run_share_op` (spawn_blocking + 30 s timeout) wraps
every service call, `rebuild_jails` was split into an async DB phase and a
blocking mount/account phase under a 120 s ceiling. Tests: `shares.rs`
14 → 34 unit, `sftp.rs` 6 → 9 unit, `shares_integration.rs` 20 → 26
integration (`cargo test` lib 942 → 965; the three neighbouring share suites
— `resource_shares`, `share_links`, `api_contract` — plus `users`, `groups`
and `mdns` re-run green). Docs: `docs/guides/shares.md` gained the validation
table, the exports-rewrite semantics and the blocking-pool invariants;
`docs/API.md`'s Shares section documents the validation and the toggle
semantics. The live-host pass (real SMB/NFS/vsftpd reloads, the disabled
export surviving a reboot, an ambiguous-basename 400 against a real
`/etc/exports`) is recorded in `TESTING.md`'s "Remaining to test" list. See
`CLAUDE.md` (2026-10-21 addendum).

**Fixed on 2026-10-21 — Network production-ready:**

The Network screen was the last one whose own headline feature stopped half
way: `PUT /network/interfaces/{name}/config` persisted the new IP configuration
(written 2026-10-20) but **never applied it** — runtime and boot state could
differ by one re-apply cycle, and the admin could not see in the session
whether the netplan file was loadable and equivalent to the wire. The request
now runs the production apply-verify-rollback pattern: capture the pre-change
state → live apply → persist (the write returns the backend *plus a snapshot
of the previous netplan file*) → `netplan apply` → verify the wire converged
within a 10 s grace (static address present, default route via the configured
gateway, static v6 present, interface up — pure verdict `check_live_state_matches`)
→ on any failure, roll back both halves (previous file restored or ours
deleted, `netplan apply` again, previous live configuration re-asserted so the
operator session survives) and answer `success:false` naming what failed.
Sudoers gained the scoped `netplan apply` / `cat` / `chmod 600` / `rm -f`
shapes for `/etc/netplan/99-homevault-*.yaml`, and `netplan.io` joined the
dependency inventory (optional — Debian hosts without it keep ifupdown, which
deliberately stays write-only/boot-applied). Tests: `network.rs` 43 → 52 unit
(9 on the verify verdict), `cargo test` full run green (lib 917), docs in
`docs/guides/network.md` + `docs/API.md`; the live reconcile pass is listed in
`TESTING.md`'s "remaining to test" (needs a real host). See
`CLAUDE.md` (2026-10-21 addendum).

**Fixed on 2026-10-21 — APPS production-ready / app-proxy auth under HTTPS:**

Apps Center's per-app reverse proxy had one real defect that only hit the
security-motivated setups: the gate redirected unauthenticated browsers to a
hard-coded `http://{main_port}/app-auth`, but under an HTTPS deployment the
`hv_session` cookie carries `Secure` and is never re-sent over plain HTTP —
the round-trip looped forever and every "require login" app was unusable
precisely under TLS. The gate's redirect is now TLS-aware
(`https://{host}:{https_port}/app-auth` while the HTTPS listener is up,
computed per request), auth-minting redirects carry `Cache-Control:
no-store`, the login screen honours the flow's `next` parameter (same-origin
paths only — no open-redirect window), and the app-session store is capped at
512 live entries with soonest-expiring eviction. `hv_app_session` deliberately
stays without `Secure` (the proxy listener is plain HTTP even under an HTTPS
dashboard). `docs/guides/apps-center.md` was rewritten to describe the
shipped launcher (Docker apps open in a new browser tab; the earlier
iframe + mixed-content-toast design was never on this path). Tests: 4 new
unit tests on the location builder, the store-cap test, and an end-to-end
TLS redirect step in `tests/app_proxy_integration.rs`. See `CLAUDE.md`
(2026-10-21 addendum) and `CHANGELOG.md` [Unreleased] → Fixed.

**Fixed on 2026-10-21 — VPN production-ready:**

The VPN module answered three classes of "it looks done but fails in
production": (a) an OpenVPN start answered "started" while `openvpn --daemon`
had merely forked — bad credentials or an unreachable endpoint left a daemon
retrying forever behind a success the admin already saw; (b) an uploaded
`.ovpn` profile was executed as root with no content check, so the exec-hook
directives (`up`, `client-connect`, `auth-user-pass-verify`, `script-security
> 1`, …) were a privilege escalation away from any compromised admin
session; (c) every VPN status poll and peer provisioning contacted three
external echo endpoints with no cache. The fix, verified empirically against
OpenVPN 2.6.19 during this pass: the start is now **start → verify →
rollback** — per-connection log + `--cd /etc/openvpn` (`--daemon` chdir()s to
`/`, and the client-side `--status` file carries no connection state — both
behaviours were confirmed on a live 2.6.19 client, not read from docs), a
10 s / 250 ms poll for the `Initialization Sequence Completed` marker, kill +
log-derived error message on timeout or fatal, idempotence when already
running, credentials kept only while the connection exists
(`/etc/openvpn/openvpn-<name>.auth`, 0600, deleted on stop/delete — never
`/run`, which the service cannot write); the profile deny list is enforced in
the engine and at the handler (400 naming the directive, 409 on replacing a
running profile); `ensure_wireguard_server` verifies `wg0` came up after
`wg-quick up`; all VPN mutation handlers run through `spawn_blocking`; and
`discover_public_ip` is cached (`PublicIpCache`, 15 min positive / 60 s
negative TTL, injected-instant unit tests). Frontend: per-card connecting
spinner on the OpenVPN start (`vpn.connecting` ×4 locales). `docs/guides/
vpn.md` rewritten — the previous text still described the pre-roadwarrior
`/wireguard/keygen` + `/{name}/start` endpoints — and `docs/API.md` lists the
real routes. Tests: `vpn.rs` unit 14 → 29, `vpn_integration.rs` 22 → 25; the
real tunnel pass (WireGuard handshake from a phone, an OpenVPN auth failure
watched through the verify) is recorded in `TESTING.md`'s "Remaining to
test" list. See `CLAUDE.md` (2026-10-21 addendum).

**Fixed on 2026-10-20 — Docker production-ready:**

The Docker module was audited end to end and every gap closed in one pass:
**volumes** (list/create/delete, in-use detection), **disk usage**
(`docker system df`) with **prune** (images / volumes / builders / system —
unknown targets rejected before the daemon is contacted), **container
inspect**, **exec**, **pause/unpause**, **kill** (whitelisted signals),
**rename**, **in-place limit updates**, **update to the newest image**
(pull + recreate with rename rollback), **live log streaming** over a
single-use SSE ticket (merged stdout+stderr, tail size, timestamps, follow),
**image builds** from a path-guarded host folder, **private registries**
(`--password-stdin`, credentials never read back), a **`daemon.json`
editor** validated before it is written, and **Compose update**
(`compose pull` + `up -d`). Two systemic defects went with them: no Docker
CLI call had a timeout (a wedged daemon parked a Tokio worker forever) and
every mutation ran on the async runtime (one slow `docker` call stalled all
concurrent requests) — both fixed by `run_docker`/`wait_with_timeout` and a
`spawn_blocking` wrapper. Tests: `docker.rs` 9 → 17 unit tests, new
`tests/docker_integration.rs` (10), `npm run check` 0/0, `npm run build` OK,
and the E2E suite re-run against a live daemon — **123 passed / 9 skipped /
0 failed** across firefox + chromium, which itself caught three real UI
defects (popover menus clipped by the card's `overflow-hidden`, Escape not
closing them from the trigger, a racy count in test 5.2). See
`docs/guides/docker.md` → "Production hardening".

**Fixed on 2026-10-12, on explicit request ("we start with #1", then
"keep going" twice, then "termine"):**

- **Deleting a user left its share permissions behind (real bug, security
  relevant).** `handle_user_delete` (`src/handlers/users.rs`) only walked the
  user's effective permissions to log `"Revoking permission for user …"` —
  neither the POSIX ACL nor the `share_permissions` row was ever removed.
  Since the table keys a grantee by **name only** (no foreign key to
  `users`), the rows outlived the account: recreating a username silently
  inherited the deleted user's access, and the log claimed a revoke that
  never happened. Fixed by
  `PermissionService::revoke_all_user_permissions` (ACLs best-effort +
  `delete_for_grantee("user", …)`), called from the handler before the
  account row is dropped. Group grants stay: the account was already removed
  from its groups, and those rows belong to the remaining members.
  Regression test
  `test_user_delete_revokes_its_direct_share_permissions_only`
  (`tests/users_integration.rs`, 60 → **61**), **verified to fail on the old
  code** (`git stash`: `left: 2, right: 0`).
- **`ProgressManager::get_progress` returned a placeholder** —
  `GET /api/progress/{job_id}/status` reported `Running` / 0 % /
  `"Waiting..."` for a job at 87 %, for a completed job and for a failed one.
  Fixed to return the stored `last_event` (same snapshot as the activity feed
  and the SSE stream); unknown/removed id → `None`/400. 5 new unit tests.
- **`JobType::Custom` could not be serialized at all** (found by the
  integration test written for the item above). `#[serde(tag = "type")]`
  cannot tag a `Custom(String)` newtype variant, so `handle_progress_sse`'s
  `to_string(&event).unwrap_or_default()` sent an **empty** `data:` line for
  every feature job (Explorer copy/move, Docker install, antivirus, RAG, VM,
  rescue, cloud sync, Ollama pull): the client's `JSON.parse` threw, no
  `progress`/`done` callback fired, and `Explorer.svelte`'s `watchJob`
  therefore resolved **false** for a copy that had succeeded — frozen progress
  bar, directory listing never refreshed. `GET /api/progress/jobs` panicked
  for as long as such a job was tracked (the taskbar was spared: the activity
  handler maps `JobType` to a slug and never serializes the enum). Fixed with
  **adjacent** tagging (`content = "name"`): built-in kinds keep their exact
  previous shape, feature jobs become
  `{"type":"custom","name":"…"}`. 4 new unit tests + 1 integration test;
  both fixes verified to fail on the old code.
- **Doc corrections found by the same audit** (the code was already right):
  `docs/guides/shares.md` still listed as "reported not fixed" the recursive
  grant failing and the "rw" option lacking the execute bit — both fixed in
  the code long ago (`set_default_acl`'s `-d -m` order, execute forced on a
  directory); and `docs/guides/chat.md` claimed
  `handle_chat_models_recommend` had no session check, which it now has.
- **Cloud Sync read and wrote a file it never parsed.**
  `handle_get_config` / `_save_config` / `_add_remote` / `_delete_remote`
  hardcoded `/etc/rclone.conf` while `parse_rclone_config()` honoured the
  `RCLONE_CONFIG` override: an admin whose config lives elsewhere got a
  remote list and a Settings editor describing **two different files**, and
  saving failed with `Permission denied` against the root-owned default.
  Fixed with one resolver, `rclone_config_path()`, used by the parser and by
  all four handlers. `handlers/rclone.rs` unit tests 11 → **14**,
  `tests/rclone_integration.rs` 14 → **17** — the full
  `add_remote → list → delete_remote` cycle and the `.backup.<ts>` sibling
  now run for real against a tempdir-selected config, which the hardcoded
  path made impossible. Verified to fail on the old code (`Failed to read
  config /etc/rclone.conf: No such file or directory`, `Failed to save
  config: Permission denied`).
- **The RAG knowledge base was missing 25 of the 45 indexable guides.** The
  rule "a new feature gets a `FeatureDoc` + a `rag.md` Document Sources row"
  was prose only and had never been applied retroactively, so
  `src/rag_indexers.rs` carried 20 entries against 46 guide files — the
  assistant answered "I don't know" about Syncthing, the downloader,
  notifications, the desktop shell, LUKS encryption, RAID, the cache tier,
  widgets and a dozen more shipped features. All 45 are indexed now, and the
  coverage is **enforced by a test** (`test_every_guide_has_a_feature_doc`
  enumerates `docs/guides/` and reconciles it with `FEATURE_GUIDES` in both
  directions), so the drift cannot come back silently. The old
  `test_feature_docs_are_nonempty` asserted a count on a hardcoded list
  unrelated to the shipped vector, which is why it never caught anything.
- **The 3 `manual_contains` clippy warnings in `src/dependencies.rs`**
  (`iter().any(|have| *have == want)` → `contains(&want)`), left by the
  install.sh refactor. CI runs `cargo clippy --all-targets -- -D warnings`, so
  they were a hard CI failure rather than a style note.

**Resolved since the first version of this file (verified 2026-07-21)**

For the record — these items were listed as "not fixed" in the previous
version of this roadmap and turned out to be already resolved upon
verification:

- Session flaw on `handle_group_add_member` (Groups) — already fixed,
  validates the session for every caller.
- `PermissionService::grant_permission` in recursive mode — already fixed
  (`setfacl -d -m` argument order).
- "rw" option without the execute bit on shares — already fixed
  (execution forced on directories).
- UPS/SNMP without a UI — already resolved, `UpsCard.svelte`/
  `SnmpCard.svelte` exist and are mounted in `Settings.svelte`.
- Orphaned old Settings implementation — the files
  (`SettingsLayout.svelte`, etc.) no longer exist at all.
- Dead HTML handlers in `dashboard.rs` — the functions themselves have
  been removed.

**Fixed on 2026-07-21, on explicit request ("fix the 3 items"):**

- **Desktop shell i18n** — the ~19 app names/descriptions
  (`appDefs`/`apps` in `Desktop.svelte`) and the global search index now
  go through dedicated keys `desktop.app_<id>_name`/`_desc` (4
  languages), following the scheme already established for the Docker
  catalog and the widgets. The "AI Assistant" entry in search, the only
  one that had remained in plain French while the rest was in English
  (the exact "EN/FR mix" bug described in `CLAUDE.md`), is fixed as
  well.
- **Transfer quotas overview** — new "All quotas" button in the header
  of the Users page, opening a modal listing all configured quotas (users
  and groups) via `api.listTransferQuotas()` (new method, reads the
  already existing list endpoint). Verified live: real quota configured
  for a user, displayed correctly in the table.
- **Minor cleanup** — the dead i18n keys (`two_factor`,
  `backup_codes*`, old `general`/`email`/`network`/`about` labels)
  removed from the 4 TOML files. The `security` key of that same group
  was **kept**: verification showed it is actually used by
  `Settings.svelte` as the label of the "Security" category of its own
  sidebar menu — so it was not dead, contrary to what `CLAUDE.md`
  suggested.

## Process

- Any change to an audited screen must keep its tests green
  (Rust + Playwright E2E) — see `CLAUDE.md` → "Design system
  governance".
- New verification reports go into dated addenda in `CLAUDE.md`, not into
  a separate file (the "Keep the repository clean" rule at the top of
  `CLAUDE.md`).
- **Before adding an item to this file from the text of `CLAUDE.md`,
  verify against the current code (direct reading of the file concerned,
  not just `grep`/memory) that it is still relevant** — that is precisely
  what was skipped during the first version of this file and what produced
  a roadmap that was 85% wrong.
- This file must be updated on every bug found-but-not-fixed or incomplete
  feature identified during a future audit, and purged of the items that
  end up being fixed or abandoned.
