# HomeVault Roadmap

Last updated: 2026-10-19 — current version: 1.0.0

## Current state

HomeVault today covers: dashboard, users/groups, storage
(disks/partitions/ZFS-Btrfs-mdadm virtual disks/LUKS2
encryption/SMART health), RAID (mdadm/ZFS/Btrfs creation), shares
(SMB/NFS/FTP/WebDAV/SFTP), file explorer, backups (restic), cloud sync
(rclone), network, security
(antivirus/anti-ransomware/firewall), web terminal, VPN (WireGuard/
OpenVPN), downloader (aria2), Syncthing, Docker + application catalog,
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
