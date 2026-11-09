# HomeVault — Test tracking

## 2026-11-09 — DSM-parity verification fixes (written, not run)

> **No Rust, integration, E2E or installer suite was executed for this pass**,
> per the maintainer's instruction. Static checks only: `bash -n` on
> `installer/install.sh` and both installer test scripts, `cargo check --lib`,
> `cargo check --tests`.
>
> The fixes below close the 23 failures reported by the first full
> `cargo test --lib` run on `910a84f` (2 804 passed / 23 failed). Two of them
> were functional blockers, not test drift: the media catalog rejected every
> absolute `share_mount`, and the GPU block was injected at column 0.

| Suite | Delta | Coverage |
|---|---|---|
| Unit (`src/app_catalog/manifest.rs`) | existing | `share_mount` accepts `/media` and `/mnt/media`; refuses relative, root, trailing slash, traversal, whitespace, `:`, and `//` |
| Unit (`src/app_catalog/mod.rs`) | existing | `replace_block` indents every injected line (GPU `devices:` under its service) and drops a removed line cleanly |
| Unit (`src/docker.rs`) | matrix extended | valid: `redis`, `redis:7`, `ghcr.io/acme/app:1.2`, `localhost:5000/app`, `registry.example.com:5000/team/app:1.2.3`; refused: `redis:tag/slash`, `redis:7:8`, `registry:abc/app`, `registry:0/app`, `/app`, `app/` |
| Unit (`src/downloader_rss.rs`) | existing | Atom `<link rel="enclosure" length="2048">` yields `size = Some(2048)` |
| Unit (`src/email_oauth.rs`) | existing | a redirect URL with an internal space is refused before registration |
| Unit (`src/host_accounts.rs`) | +1 | `samba_sync_is_noop` recognises no-entry, non-root usage and permission denial; real failures stay errors |
| Unit (`src/handlers/journal.rs`) | extended | LF and CRLF in a message become one space; the CSV has exactly header + one record per entry |
| Unit (`src/rag_indexers.rs`) | +2 FeatureDocs | `email-oauth` and `remote-mounts` are indexed; every guide still maps to exactly one `FeatureDoc` |
| Unit (`src/dependencies.rs`) | +1 spec, 2 tests rewritten | `bash` is a base binary; `net`/`smbpasswd` are probed under `samba`; the rendered unit is executed via `render_service_unit` (root and unprivileged modes); bootstrap flags include `--verify-manifest` |
| Unit (`src/setup.rs`) | rewritten | the `incus export/import` grants are scoped to `base_dir()/vm/exports`, not a hardcoded `/srv/homevault` |
| Unit (`src/shares.rs`) | rewritten | `server` remains refused; `ads`/`domain` require a realm and a valid idmap and are accepted with both |
| Unit (`src/ws_discovery.rs`) | corrected | a newline in the NetBIOS/workgroup input cannot add a directive and truncates to 15 characters |
| Installer (`installer/tests/update-engine.test.sh`) | +1 assertion | `history.jsonl` has exactly one line per result and that line parses with `json.loads` |
| E2E (`tests/e2e/docker/docker.spec.ts`) | unchanged | 15.1/15.3b already cover installing a media app on a share with GPU opt-in; **not re-run** |

**Still to run before calling these "delivered"** (`COMPARAISON_DSM_VS_HOMEVAULT.md` §6.4): `cargo test` (unit + the 109 integration suites), `cargo fmt --check`, `npm run check && npm run build`, the targeted E2E specs, and the two installer suites.

## 2026-11-09 — integrity lot merged into main (written, not run)

> **No test was executed for the merge.** Static checks only:
> `cargo check --lib`, `cargo check --tests`, `bash -n installer/install.sh`,
> `npm run check` (0 errors / 0 warnings).
>
> The reliability lot `d46a2a5` (P0-5→P0-12, P2-10→P2-14) is now in `main`.
> It had never met the 26 newer commits, so the merge resolved both code and
> data-shape conflicts; the full suite must confirm the result on a host.

| Area | Merge decision / coverage |
|---|---|
| Scheduler | union of the 7 task types (`command`, `snapshot`, `smart_test`, `scrub`, `restic_check`, `power`, `service`); new unit test `all_seven_task_types_coexist_after_the_integrity_merge` |
| Migrations | colliding 082–085 from the lot renumbered **089–092**; **093** rebuilds the `scheduled_tasks` CHECK with all 7 types and copies the run history |
| SMB config | `realm: String` (P2-1 shape), reliability fields (durable handles, encrypt, signing, strict sync, AIO, oplocks) kept; i18n duplicate keys collapsed |
| iSCSI | `ValidatedBackstore` struct kept; zvol creation/clone carry write policy/FUA/UNMAP; UI keeps zvol + snapshots + I/O policy |
| Btrfs | `pools.rs` uses `btrfs_maintenance::fstab_options`, no duplicated `compress=zstd` strings |
| Installer | `render_service_unit` (P2-7) kept, lot's static heredoc discarded |
| Shares UI | ADS/join block + WS-Discovery + reliability toggles in one card |
| Tests realigned | `nfs_export_line` now takes the client-rules slice; `SmbConfig` test literals carry the union of fields |

**Still to run:** `cargo fmt --check`, `cargo test` (unit + 109 integration
suites), the E2E specs for Shares/iSCSI/Tasks/Storage/Health, the two
installer suites, and `npm run build`.

## 2026-11-09 — systemd watchdog heartbeat (FIAB-02, written, not run)

> **No test was executed.** Static checks: `cargo check --lib`,
> `cargo check --tests`, `bash -n installer/install.sh` and
> `bash -n installer/tests/unit-hardening.test.sh`.

| Suite | Coverage |
|---|---|
| Unit (`src/systemd_notify.rs`) | newline-terminated `sd_notify` wire shape; pathname and abstract (`@name`) destinations; `notify`/`spawn_heartbeat` are no-ops without `NOTIFY_SOCKET`; a real `UnixDatagram` receives `WATCHDOG=1` and `READY=1` |
| Unit (`src/dependencies.rs`) | the rendered unit contains `WatchdogSec=30` and `NotifyAccess=main` |
| Installer (`installer/tests/unit-hardening.test.sh`) | +2 assertions for the watchdog directives in the root unit |

**Still to run:** the unit tests above, the installer suite, and a live
`systemctl stop`/`kill -9` on a host to prove the restart path. The per-loop
supervisor is covered in the next section.

## 2026-11-09 — background-loop supervisor (FIAB-02, written, not run)

> **No test was executed.** Static checks: `cargo check --lib`,
> `cargo check --tests`, `npm run check` (0 errors / 0 warnings).

| Suite | Coverage |
|---|---|
| Unit (`src/scheduler_supervisor.rs`) | backoff doubles and caps at 300 s, resets after 60 s; a returning loop is recorded and restarted; a panicking loop becomes `JoinError` and is restarted |
| Unit (`src/handlers/scheduler_health.rs`) | admin-only route, JSON array payload (via integration below) |
| Integration (`tests/health_integration.rs`) | `/api/health/schedulers` is 401 without a session, 403 for a member, 200 for an admin; a registered loop appears in the list |
| Frontend (`Health.svelte`) | admin-only **Background schedulers** card with running/stopped, restart count and last error |

**Still to run:** the three tests above and a live `kill -9` of one loop to
confirm the restart/backoff path.

## 2026-11-09 — data directory on a redundant pool (FIAB-04, written, not run)

> **No test was executed.** Static checks: `bash -n installer/install.sh`,
> `bash -n installer/tests/data-dir.test.sh`.

| Suite | Coverage |
|---|---|
| Installer (`installer/tests/data-dir.test.sh`) | `validate_data_dir` accepts `/srv/homevault`, `/mnt/pool/homevault`, `/var/lib/homevault`; refuses empty, relative, `/`, whitespace, `..`, `//` with a named reason; `on_system_filesystem /` is true and `/proc` is not |
| Installer behaviour | `HV_DATA_DIR` wins over `DATA_DIRECTORY`; an invalid value falls back to `/var/lib/homevault` with a warning; the warning when the directory shares `/`'s filesystem names the remedy |

**Still to run:** the installer suite, then the FIAB-05 bare-metal rebuild on a
disposable VM (remove the system disk, install fresh, restore DB + config from
an off-machine copy, re-attach the pool, measure RTO/RPO).

## 2026-11-09 — RAID resync speed control (FIAB-10, written, not run)

> **No test was executed.** Static checks: `cargo check --lib`,
> `cargo check --tests`, `npm run check` (0 errors / 0 warnings).

| Suite | Coverage |
|---|---|
| Unit (`src/raid_stability.rs`) | saved profile round-trips; an unknown stored profile parses to none (loader falls back to `low`); existing profile/limit matrix kept |
| Integration (`tests/storage_integration.rs`) | GET/PUT `/api/storage/raid/resync-speed` are admin-only (member 403); GET returns `low`/`balanced`/`fast`; an unknown profile is a `400` before any sysctl write |
| Frontend (`Storage.svelte`) | admin-only selector, save action, `applied`/pending notice, 4 locales |

**Still to run:** the tests above plus a real rebuild on an md array to verify
the sysctls survive a reboot and the shares stay responsive under the `low`
profile.

## P1-28 — kernel stability + explicit monitoring choice — 2026-11-09 (not run)

> **No Rust or E2E test was executed for this change.** Static checks passed:
> `bash -n installer/install.sh` and
> `bash -n installer/tests/unit-hardening.test.sh`.
> **No host setting is changed by the tests**: `render_sysctl` and
> `monitor_decision` are pure renderers/decisions.
> The installer unit file was run once during development to confirm the new
> assertions: **55 passed, 0 failed**.

| Coverage | Added / changed |
|---|---|
| Installer (`installer/tests/unit-hardening.test.sh`) | **+9 assertions** — `render_sysctl` contains the panic/oops/dirty-ratio values and no unexpanded substitution; `monitor_decision` returns `unavailable` (missing package), `already` (enabled wins over the request), `enable` (explicit `HV_*=1`) and `leave` (default) |
| Installer (`installer/install.sh`) | writes and applies `/etc/sysctl.d/99-homevault.conf` (`HV_SKIP_SYSCTL=1` skips applying), and the `smartd`/`mdmonitor` block enables only on request. `bash -n` only |
| Docs | `docs/ARCHITECTURE.md` (new "Kernel stability and host monitoring" section), `CHANGELOG.md`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-28), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1-27 — signed community app stores — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check`, `npm run build`, `bash -n` on the installer
> and `npx playwright test --list`.
> **No git repository is cloned and no real key is used by the tests**: the
> unit tests generate deterministic Ed25519 keypairs in memory, and the
> integration test only asserts the reported state.

| Coverage | Added / changed |
|---|---|
| Unit (`src/catalog_signature.rs`) | **+7** — hex/base64 decoding (whitespace tolerated), 32-byte key length, Ed25519 verification accepts only the matching key/message/signature, a missing or tampered `.sig` is reported, the checkout policy (no key → Unconfigured, require → error, keyed → Verified, unsigned app refused, wrong key refused), `state_for_store` folding, and a stable 16-char key fingerprint |
| Unit (`src/docker.rs`) | **+3** — `image_reference` builds `name:tag`, keeps a `@sha256:` digest unchanged (never appends `:latest`) and defaults an empty tag; malformed digests/tags/characters are refused; `unpinned_images` names every service missing a valid digest |
| Integration (`tests/app_catalog_integration.rs`) | **+1** — `GET /docker/stores` reports `unconfigured` (with `app_count` 1) when no key is set, and `invalid` + the reason + `app_count` 0 under `HV_REQUIRE_SIGNED_STORES=1`; the refused store's app is absent from the catalog |
| Installer (`installer/install.sh`) | `HV_CATALOG_PUBKEY` is forwarded through the sudo re-exec and installed at `/usr/local/lib/homevault/catalog-key.ed25519` (path or inline key); help text documents it. Syntax checked with `bash -n` (not executed) |
| Docs | `docs/guides/docker.md` (new "Signed community stores" section with the signing commands), `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `docker`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-27), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1-16 — SMTP OAuth2 (Gmail / Microsoft 365) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check`, `npm run build` and
> `npx playwright test --list`.
> **No test reaches a provider**: every Rust case stops before the token
> endpoint, and the E2E suite never clicks Connect.

| Coverage | Added / changed |
|---|---|
| Unit (`src/email_oauth.rs`) | **+11** — provider parsing/defaults (Google/Microsoft, unknown stored value falls back, strict request parse); Google URL requests `access_type=offline` + `prompt=consent`; Microsoft uses its own endpoint/scope; bad client id / redirect / state refused; redirect validation (http(s), no credentials, no fragment); client id/secret validation; token parsing (rotation, missing/absurd `expires_in`, bounded token-free errors); the public status carries no sealed value; configured vs connected; state is single-use |
| Unit (`src/email.rs`) | **+3** — the msmtp config uses `oauthbearer`/`xoauth2`, quotes the access token, refuses an unknown mechanism or a control character in the user/token, and requires TLS (a bearer token must never go out in clear) |
| Unit (`src/handlers/email_oauth.rs`) | **+3** — the callback echo is HTML-escaped, the result page is `text/html` + `no-store`, and the state lifetime is short |
| Integration (`tests/email_oauth_integration.rs`) | **+6** — GET/PUT/DELETE/start are admin-only; validation (provider, redirect scheme, client id) and the response never carries `client_secret`/`refresh_token`/`hv1:`; the file on disk is sealed and not plaintext; start returns the provider URL + HttpOnly/SameSite cookie; the callback refuses a missing/mismatched/replayed state without any network call; disconnect clears the file |
| E2E (`tests/e2e/settings/settings.spec.ts`) | **+6** — §31.1 labelled card and unconfigured state; §31.2 save makes it ready without echoing the secret; §31.3 invalid redirect message; §31.4 start URL + state; §31.5 admin-only status; §31.6 disconnect confirmation |
| Docs | `docs/guides/email-oauth.md` (new), `docs/INDEX.md`, `docs/guides/settings.md`, `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `settings`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-16), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1-18 UI — structured filters in the System Journal — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check`, `npm run build` and
> `npx playwright test --list`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/handlers/journal.rs`) | **+2** — `structured_filter_used` names the first offending field (from/to/severity/actor/action) and treats blank values as absent; `audit_query_from_journal` trims every filter, drops blanks and carries the limit/offset |
| Integration (`tests/journal_integration.rs`) | **+5** — the audit source filters by actor/action/severity and returns the structured fields; the date range is inclusive and `offset` pages the same ordered set with `meta.total`; a structured filter on `app`/`system` is a `400` naming the field (and so is the export); a malformed date, an unknown severity and an inverted range are `400`; the export honours the structured filters and still combines with the free-text search |
| E2E (`tests/e2e/journal/journal.spec.ts`) | **+5** — §13.1 the filter panel appears only on the audit source and every control has a label; §13.2 every filter reaches the request with `offset=0`; §13.3 Clear removes them; §13.4 an inverted range shows the error and sends no request; §13.5 the CSV export carries the same filters |
| Docs | `docs/guides/journal.md` (new "Structured filters in the Journal screen" section), `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `journal`, `HOMEVAULT_TASKLIST_A_FAIRE.md`, `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2-8 UI half — apply a signed update from Settings — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check`, `npm run build` and `bash -n` on the
> installer and its tests.
> **No update is applied by any test**: the engine is not installed in the test
> environment, and every case is refused before a privileged call.

| Coverage | Added / changed |
|---|---|
| Unit (`src/update_engine.rs`) | **+5** — manifest validation (`https://`/absolute path only, spaces/`..` refused); signature and trust-anchor validation; request and engine-exit round-trip with forward-compatible defaults; log tail is bounded and a missing log is empty (not an error) |
| Unit (`src/setup.rs`) | **+1** — the update grant is exactly `systemctl start homevault-update`; `stop`/`restart` and a wildcard are refused |
| Installer (`installer/tests/unit-hardening.test.sh`) | **+7 assertions** — the rendered update unit has the fixed `ExecStart`, is `oneshot`, needs the installer script, survives the restart (`KillMode=process`), is time-bounded, and carries no shell or unexpanded substitution |
| Integration (`tests/settings_core_integration.rs`) | **+3** — the status route is admin-only and honest on a fresh appliance; the release channel is validated (`http`/`file://`/bare host refused) and persisted, and empty clears it; apply refuses a missing confirmation and a bad manifest **before** the capability probe |
| E2E (`tests/e2e/settings/settings.spec.ts`) | **+2** — §30.1 the channel field saves through the normal Save button; §30.2 the Apply button is enabled only with an engine and a channel, otherwise disabled with the reason, and the status route is admin-only |
| Docs | `docs/guides/settings.md` (new "Apply from the web UI" section), `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `settings`, `HOMEVAULT_TASKLIST_A_FAIRE.md`, `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2 Calendars/contacts in the catalog (Baïkal) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Catalog | **+1 package** (`catalog/baikal/`): manifest with `[security] no_new_privileges`, four localized descriptions, and a compose template with two separate volumes (`config`, `data`) and memory/CPU limits |
| Unit (`src/app_catalog/mod.rs`) | the built-in count assertion moves 15 → 16; the existing `test_every_builtin_manifest_declares_security`, `test_builtin_apps_render` and `test_builtin_media_apps_declare_share_mount_and_gpu_support` now also cover Baïkal (manifest parses, `[security]` present, no placeholder left after render, port bound) |
| Integration (`tests/app_catalog_integration.rs`) | the catalog list assertion moves 15 → 16 |
| E2E (`tests/e2e/docker/docker.spec.ts`) | §15.2's comment no longer hardcodes a count (the badge/install assertions stay) |
| Docs | `SCREENS.md`, `docs/guides/docker.md`, `docs/API.md`, `CHANGELOG.md`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2‑20), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2 Syncthing file versions (policy + restore) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.
> **The daemon is never contacted by these tests**: every integration case is
> refused before the first proxied request, and the E2E suite mocks it.

| Coverage | Added / changed |
|---|---|
| Unit (`src/syncthing_versions.rs`) | **+6** — the `versioning` object shape per mode (params as **strings**, `maxAge` in seconds); config round-trip and an `external` versioner reading as off; policy bounds (0/5000 versions, 99999 days); version list parsing (newest first, broken rows skipped, non-list refused); restore validation (traversal, absolute path, backslash, invalid stamp); mode parsing |
| Integration (`tests/syncthing_integration.rs`) | **+3** — the policy route is admin-only and validates the mode and the bounds before the daemon; restore validates the file and the stamp before the daemon; the versions list is admin-only |
| E2E (`tests/e2e/syncthing/syncthing.spec.ts`) | **+1** — §15.1 the Versions dialog shows the reported policy (mode, keep, cleanout) and lists an archived copy with its restore action |
| Docs | `docs/guides/syncthing.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `syncthing`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2‑19), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2 VM export / import (portable archive) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.
> **No `incus` command is executed by these tests**: the archive name is
> validated before the capability probe, the listing only reads the export
> folder, and the export/import paths stop at validation or at the missing CLI.

| Coverage | Added / changed |
|---|---|
| Unit (`src/vm_export.rs`) | **+5** — archive-name derivation (sanitised, bounded, compressed or not); name validation (traversal, absolute, hidden, wrong extension refused); path confinement; exact `incus export`/`import` argv for both shapes; a listing that ignores foreign files |
| Unit (`src/setup.rs`) | **+1** — the four scoped `incus` grants (export plain/compressed, import with/without a name) and no bare binary |
| Integration (`tests/vm_integration.rs`) | **+3** — the listing is admin-only and starts empty; import/delete validate the archive name first; export refuses a bad snapshot before touching the host (and names the missing CLI otherwise) |
| E2E (`tests/e2e/vm/vm.spec.ts`) | **+2** — §11.1 the dialog opens and reports its state honestly (works without Incus); §11.2 the API is admin-only, returns an array of file names and refuses a traversal |
| Docs | `docs/guides/vm.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `virtual-machines`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2‑18), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2 iSCSI thin LUNs on ZFS (snapshots + clones) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.
> **No `zfs` command is executed by these tests**: the integration cases use a
> pool name that cannot exist, and the snapshot/clone cases run against a
> file-backed backstore, which those endpoints refuse by design.

| Coverage | Added / changed |
|---|---|
| Unit (`src/zvol.rs`) | **+6** — name/snapshot-name validation; dataset + device round-trip (`is_managed_dataset`, a sibling dataset refused); size bounds; exact `zfs create -p -s -V` / `snapshot` / `clone` / `destroy [-r]` argv; `zfs list` parsing for volumes (thin detection) and snapshots |
| Unit (`src/setup.rs`) | **+2 assertions** — `zfs create *` and `zfs clone *` are per-subcommand grants |
| Integration (`tests/iscsi_integration.rs`) | **+4** — a zvol needs an existing ZFS pool (missing, unknown and unsafe names); a file backstore refuses snapshot list/create/clone with a named error; snapshot names are validated before anything runs; a backstore cannot become a zvol in place |
| E2E (`tests/e2e/storage/iscsi.spec.ts`) | **+3** — §7.1 the ZFS option appears with a pool and sends pool + size (no path); §7.2 absent without a pool; §7.3 the snapshot panel lists a snapshot and clones it into a new disk |
| Docs | `docs/guides/iscsi.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `iscsi`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2‑17), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2 ZFS read cache / separate log (L2ARC / SLOG) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.
> **No `zpool add`/`zpool remove` is executed by these tests**: the integration
> and E2E cases only drive the read-only listing and the refusals.

| Coverage | Added / changed |
|---|---|
| Unit (`src/zfs_cache.rs`) | **+8** — `zpool status -P` parsing (pool/state, no devices, logs vs cache, error counters, a data vdev ignored, mirrored log group, no pool header); device path shape; the refusal matrix (in use, missing, already attached, second group, spinning disk for both roles); exact `zpool add`/`remove` argv; role parsing and vdev keywords |
| Unit (`src/setup.rs`) | **+2 assertions** — `zpool add *` and `zpool remove *` are per-subcommand grants |
| Integration (`tests/zfs_cache_integration.rs`) | **+3** — admin gate on every route + listing shape; role/device validation before any `zpool` call; detach refuses a non-`/dev` device |
| E2E (`tests/e2e/storage/storage.spec.ts`) | **+2 tests, +1 assertion** — §14.1 the card reports exactly one host state and never offers a dead Attach; §14.2 the API refuses an unknown role and a bare device name; §4.3 now also asserts a member gets `403` on `POST /api/zfs-cache` |
| Docs | `docs/guides/cache.md`, `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `cache`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2‑16), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 Downloader — RSS, per-task limits, automatic extraction — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.
> **No aria2 daemon is spawned and no feed is fetched by these tests**: the
> integration cases are refused before any RPC/network call, and the E2E suite
> only opens the dialogs.

| Coverage | Added / changed |
|---|---|
| Unit (`src/downloader_rss.rs`) | **+7** — feed URL shape and SSRF guard (loopback, RFC 1918, IPv6 loopback, `user:pw@`, non-http schemes); RSS parsing keeps only downloadable items (news-only and `file://` dropped); Atom enclosure/`rel=alternate`; malformed/empty documents; magnet vs `https://` shape; the `seen` marker queues once and stays bounded (500); default name/label; store round-trip and forward-compatible defaults |
| Unit (`src/downloader_extract.rs`) | **+4** — archive allowlist matches the installed tools (`.rar`/`.7z` refused, `.zip.part` refused); exact `unzip`/`tar` argv (argv only, `--no-absolute-names`, download folder as target); a folder with no archive is a no-op; settings default off and round-trip; the processed marker stays bounded |
| Unit (`src/handlers/downloader.rs`) | **+3** — `validate_rate_limit` (K/M/G only, ≤ 9 digits, `-5`/`1.5M`/`5T` refused), `validate_seed_ratio` (0–100), and the existing confinement matrix unchanged |
| Integration (`tests/downloader_integration.rs`) | **+4** — limits refused before aria2; `/settings` default off + round-trip + admin gate; RSS CRUD with the SSRF guard, duplicate refusal and admin gate; refresh/extract refused before any call |
| E2E (`tests/e2e/downloader/downloader.spec.ts`) | **+2** — §15.1 the RSS dialog, its safe defaults and the refused private feed URL; §15.2 the optional per-task speed limit field |
| Docs | `docs/guides/downloader.md`, `docs/API.md`, `docs/guides/scheduled-tasks.md` (new service), `SCREENS.md`, `PRIVACY.md` (outbound table + RoPA 19), `CHANGELOG.md`, RAG `downloader`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1‑36), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 Explorer — network locations (CIFS/NFS) + Btrfs versions — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.
> **No mount is ever executed by any of these tests.** The integration suite
> drives only the refusal paths and "save without connecting"; the E2E suite
> opens the dialog and reads the API.

| Coverage | Added / changed |
|---|---|
| Unit (`src/remote_mounts.rs`) | **+10** — name/server/source validation (traversal, spaces, `:`/IPv6, `-o…`, path-in-share); per-protocol option allowlist (credentials/`rw`/`suid` refused, duplicates collapse, cap); account fields cannot inject a credentials key; exact `mount`/`umount` argv for CIFS and NFS; credentials body; `/proc/mounts` parsing with octal escapes and segment boundaries; `target_for` derives from the name only; store round-trip + forward-compatible defaults |
| Unit (`src/snapshots.rs`) | **+2** — `deepest_containing_root` respects segment boundaries (`/srv/photos` vs `/srv/photos2`); `version_path_in_snapshot` appends only the part below the share and refuses a sibling folder or an empty root |
| Integration (`tests/remote_mounts_integration.rs`) | **+7** — admin gate (401/403/200 shape); unsafe name; injected server, path-in-share; credential option; save-without-connecting (or `503` naming the helper) + duplicate refusal + delete; name validated before any syscall; reconcile with nothing configured |
| E2E (`tests/e2e/explorer/explorer.spec.ts`) | **+3** — §19.1 the panel and its Add dialog (safe defaults, skips when no helper); §19.2 an unsafe name is refused with the rule inline; §19.3 the API never leaks a password |
| Docs | `docs/guides/remote-mounts.md` (new), `docs/guides/explorer.md`, `docs/INDEX.md`, `docs/API.md`, `PRIVACY.md` (RoPA ligne 18), `SCREENS.md` (§16/16c), `CHANGELOG.md`, RAG `explorer`/`snapshots`, `installer/install.sh` + dependency registry (cifs-utils/nfs-common), `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1‑35), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 media integration (share binding + hardware transcoding) — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/app_catalog/manifest.rs`) | **+1** — `share_mount` accepted for `/media` and `/mnt/media` (default folder `media` / `mnt-media`), refused for relative, root, trailing slash, `..`, whitespace, `:` and empty components |
| Unit (`src/app_catalog/mod.rs`) | **+7** — built-in media apps declare `share_mount`/`gpu`; a chosen share renders `host:/media:ro` and is *not* treated as app data; the default folder lives under `apps/<key>` and *is* created; the GPU block follows the device and the opt-in (and never appears for an app without `gpu`); an invalid share path is refused by the renderer; a `{{SHARE_DIR}}` placeholder without a manifest declaration is an error; `replace_block` re-indents and drops whole lines |
| Unit (`src/handlers/docker_catalog.rs`) | **+2** — the marker round-trips `environment`/share/`gpu` and rebuilds the same `RenderOptions` (a vanished share falls back to the recorded path); a legacy marker with none of the new keys still loads with defaults |
| Integration (`tests/app_catalog_integration.rs`) | **+3** — a share is refused for an app that does not declare one; an unknown share type and an unconfigured share are `400`s (never an arbitrary bind mount); GPU is refused for an app without hardware support |
| Integration (`tests/mcp_integration.rs`) | **+4 assertions** — the install review now exposes `share_mount`, `gpu`, `gpu_available`, `shares` |
| E2E (`tests/e2e/docker/docker.spec.ts`) | **+1 test, +4 assertions** — 15.1 checks the review fields; 15.3b checks the Library selector defaults to the app's own folder and the GPU checkbox is opt-in (disabled with the reason when the host has no `/dev/dri`) |
| Docs | `docs/guides/docker.md` (new "Media integration" section), `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `docker` FeatureDoc, `HOMEVAULT_TASKLIST_A_FAIRE.md` (§3.2 P1 card), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 WireGuard remote-access UX — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/vpn.rs`) | **+2** — endpoint host validation (IPv4/IPv6/DNS accepted, scheme/port refused); the warning matrix (unset, RFC 1918, CGNAT, loopback, IPv6 ULA/link-local, public IP/DNS) |
| Integration (`tests/vpn_integration.rs`) | **+1** — GET/PUT admin gate, invalid value `400`, manual override persists, clearing returns to automatic (env public IP), a LAN address is accepted and warned |
| E2E (`tests/e2e/vpn/vpn.spec.ts`) | **+1** — 1.5 the endpoint card resolves, shows the source badge, and Change opens the editor |
| Docs | `docs/guides/vpn.md`, `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `vpn` FeatureDoc, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-33), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 quota history/alerts + Journal export — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/quota_history.rs`) | **+2** — the crossing rule (first sample, crossing, staying above, below), and percent bounds for a missing/negative limit |
| Integration (`tests/quotas_integration.rs`) | **+1** — four samples produce exactly two alerts (95 → 96 → 50 → 95), the history endpoint returns them newest-first, and it is admin-only |
| Unit (`src/handlers/journal.rs`) | **+2** — CSV quoting for commas/quotes/newlines, and the header on an empty export |
| Integration (`tests/journal_integration.rs`) | **+1** — audit export CSV + JSON, admin gate, invalid format and unknown source |
| E2E | **+2** — Quotas 4.4 (row opens the history dialog), Journal 3.1b (Export requests the current filtered CSV) |
| Migration (`088_transfer_quota_history.sql`) | history table + subject/date indexes |
| Docs | `docs/guides/transfer-quotas.md`, `docs/guides/journal.md`, `docs/API.md`, `docs/BACKEND.md` (migration range), `SCREENS.md`, `PRIVACY.md` (RoPA row 17), `CHANGELOG.md`, RAG `quotas`/`journal`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-32), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 scheduled tasks — service triggers — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/scheduled_tasks_scheduler.rs`) | **+3** — the known services build the right action; a notification requires and bounds its message; unknown services and a stray message are refused |
| Integration (`tests/scheduled_tasks_integration.rs`) | **+1** — a `service` task round-trips its payload; an unknown service and a notification without a message are `400` |
| E2E (`tests/e2e/tasks/tasks.spec.ts`) | **+1** — 3.5 the type select offers a service trigger and swaps to the service picker |
| Migration (`087_scheduled_service_tasks.sql`) | rebuilds `scheduled_tasks` + `scheduled_task_runs` to widen the type CHECK; run history is copied, never cascaded away |
| Docs | `docs/guides/scheduled-tasks.md`, `docs/API.md`, `SCREENS.md`, `docs/BACKEND.md` (migration range), `CHANGELOG.md`, RAG `scheduled-tasks` FeatureDoc, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-31), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 scheduled Docker/catalog updates — 2026-11-09 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/docker_update_scheduler.rs`) | **+5** — disabled never runs; first enable sets a baseline; due/not-due at the boundary; interval bounds 6–168; the default is off and 24 h |
| Integration (`tests/docker_update_schedule_integration.rs`) | **new suite, 3 tests** — admin gate + default off; PUT persists and rejects 5/169; `run-now` is admin-gated (the admin pass is not executed against real Docker) |
| E2E (`tests/e2e/docker/docker.spec.ts`) | **+1** — 15.6 the Scheduled updates disclosure is visible, and opening the catalog never calls `run-now` |
| Docs | `docs/guides/docker.md`, `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `docker` FeatureDoc, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-30), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1 SMB WS-Discovery — 2026-11-08 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/ws_discovery.rs`) | **+6** — NetBIOS sanitization/truncation/fallback, workgroup fallback, unit body carries name/workgroup/sandbox, a newline cannot create a second directive |
| Unit (`src/shares.rs`) | **+1** — a legacy `smb.json` without `ws_discovery` defaults on |
| Integration (`tests/shares_integration.rs`) | **+1** — `test_ws_discovery_status_requires_admin` (401 / 403 / 200 + the four status fields) |
| E2E (`tests/e2e/shares/shares.spec.ts`) | **+1** — 4.9b the SMB card offers the toggle and names the real state (available explanation or the missing package) |
| Docs | `docs/guides/shares.md`, `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG `shares` FeatureDoc, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-29), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2-9 non-destructive restore drill — 2026-11-08 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib`, `cargo check --tests`,
> `cd frontend && npm run check` and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/restore_drill.rs`) | **+4** — the ready verdict requires both halves verified; newest-file selection (mtime/name tie-break); empty directory; missing checks are never ready |
| Integration (`tests/restore_drill_integration.rs`) | **new suite, 5 tests** — admin gate; a real `VACUUM INTO` snapshot + valid archive reports ready and the report persists; a corrupt DB snapshot is detected (bad header); a corrupt archive is detected; an encrypted archive is unverified without its passphrase and verified with it |
| E2E (`tests/e2e/settings/settings.spec.ts`) | **+1** — 8.6 Verify restore POSTs the drill and renders the Ready / Not ready verdict |
| Docs | `docs/guides/disaster-recovery.md` (drill + rehearsal + scope), `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, RAG FeatureDocs (`disaster-recovery`, `settings`), `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2-9), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P1-18 queryable audit trail — 2026-11-08 (not run)

> **Instruction followed: no test command was executed for this change.**
> Static checks passed: `cargo check --lib` and `cargo check --tests`.

| Coverage | Added / changed |
|---|---|
| Unit (`src/audit.rs`) | **+9** — like-pattern escaping, RFC 3339 / `YYYY-MM-DD` bounds, inverted range, severity validation, limit/offset clamps, CSV quoting, empty-CSV header, control-character stripping, character-boundary truncation |
| Integration (`tests/audit_integration.rs`) | **new suite, 5 tests** — admin gate (401/403/200), ordering + every filter + pagination, CSV/JSON export, retention prune, Journal `audit` source (admin-only, searchable) |
| E2E (`tests/e2e/journal/journal.spec.ts`) | **+1** — 2.4 the audit source is selectable and its entries request answers `200` |
| Docs | `docs/guides/journal.md`, `docs/API.md`, `docs/BACKEND.md` (migration range), `SCREENS.md`, `PRIVACY.md` (RoPA row 16), `CHANGELOG.md`, RAG journal FeatureDoc, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P1-18), `COMPARAISON_DSM_VS_HOMEVAULT.md` |

## P2-7 process isolation — 2026-11-08 (not run)

> **Instruction followed: no test command was executed for this change.** The
> shell suite was written, not run. Static checks passed: `bash -n` on the
> installer and the test file, and `apparmor_parser -Q --cache-loc=<tmp>` on the
> rendered profile (syntax valid).

| Coverage | Added / changed |
|---|---|
| Unit renderer (`installer/tests/unit-hardening.test.sh`) | **new suite** — root vs unprivileged unit; every hardening directive present; feature-breaking directives (`ProtectKernelTunables`, `ProtectKernelModules`, `ProtectHostname`, `ProtectClock`, `PrivateDevices`, `ProtectHome`, `CapabilityBoundingSet`) absent; `NoNewPrivileges` only for root; AppArmor profile attaches to the binary, defaults to complain, switches to enforce; no unexpanded command substitution. Uses `HV_LIB_ONLY=1` to source `installer/install.sh` |
| Docs | `docs/guides/security.md` → "Process isolation", `CHANGELOG.md`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2-7 `[~]`), `COMPARAISON_DSM_VS_HOMEVAULT.md` §3.1/§7 |

## P2-8 signed, atomic updates — 2026-11-08 (not run)

> **Instruction followed: no test command was executed for this change.** The
> shell engine suite and the new Rust/E2E cases below were written, not run.
> The shell suite is not part of the three Cargo/Playwright levels; run it by
> hand on a disposable host: `bash installer/tests/update-engine.test.sh`.

| Coverage | Added / changed |
|---|---|
| Update engine (`installer/tests/update-engine.test.sh`) | **new suite** — version comparison; Ed25519 signature + one-byte tamper detection; manifest fields and absent-field status; archive size/SHA-256; staged-binary smoke test (match, mismatch, crash, non-executable); atomic swap (new current, old previous, no staging file left); rollback (restore + keep failed build, refuse with no previous); result/history JSON. Uses `HV_LIB_ONLY=1` to source `installer/install.sh` without running an install step |
| Settings integration (`tests/settings_core_integration.rs`) | **+1** — `test_version_info_exposes_the_last_update_result`: `last_update` is `null` when absent, verbatim when valid, and `null` (still `200`) when the file is malformed |
| Settings E2E (`tests/e2e/settings/settings.spec.ts`) | **+1** — `8.5 — last update row is translated, never a raw i18n key` (tolerates absence on a fresh instance; asserts `/Applied|Rolled back|Failed/` and a version when present) |
| Docs | `docs/guides/settings.md` → "System → Signed, atomic updates", `docs/API.md`, `SCREENS.md`, `CHANGELOG.md`, `HOMEVAULT_TASKLIST_A_FAIRE.md` (P2-8 `[~]`), `COMPARAISON_DSM_VS_HOMEVAULT.md` §3.1/§7, FeatureDoc `settings` in `src/rag_indexers.rs` |

## P0–P3 Explorer authorization, application gate and sharing — 2026-11-08 (not run)

> **Instruction followed: no test command was executed for this change.** The
> counts below are the cases that were added or updated, not a green run. They
> must be executed before this section is marked verified. Static checks were
> run and passed: `cargo check --lib`, `cargo check --tests` (compiles the
> integration suites without running them), `cd frontend && npm run check`
> (0 errors / 0 warnings) and `npm run build`.

| Coverage | Added / changed |
|---|---|
| Explorer unit (`src/handlers/explorer.rs`) | **+1** — `every_restriction_flag_refuses_only_its_own_operation` (the browse/download/modify matrix, including preview/thumbnail as downloads) |
| Explorer integration (`tests/explorer_integration.rs`) | **+4** — read grant refused on mkdir/rename/copy/move/trash/chmod/save/compress/extract/upload; copy from a read-only source to a writable destination succeeds; listing + roots expose `permission`/`restrictions`; `app:explorer = deny` blocks list/roots/disks with `explorer_access_denied`. Harness now grants regular users `write` on the test root and adds `create_test_user_without_grant` + `grant_folder`/`grant_app` |
| Share-links unit (`src/handlers/share_links.rs`) | **+1** — target lists are trimmed/deduplicated/validated (charset, length, 32-name cap) |
| Share-links integration (`tests/share_links_integration.rs`) | **+3, ~1 updated** — creation requires a grant; targeted `signed_in` link only opens for named accounts (and targets on `anyone` are `400`); revoking the owner's grant kills an existing link; the old "directories are rejected" case became "folders are accepted and served as a tar.gz" |
| Explorer application middleware (`src/middleware/explorer_access.rs`) | **+1** — prefix covers every explorer route and not `/api/shares` or `/api/explorerx` |
| File requests (`tests/file_requests_integration.rs`) | **+6** — create requires `write`; target must be a folder; public upload lands + collision keeps both + cap returns 429; password must precede the file (missing/wrong/correct); revoking the owner grant stops uploads; unknown token renders the unavailable page |
| Frontend | `npm run check` / `npm run build` **not run**; Explorer.svelte gained `permission`/`restrictions` types, read-only guards/banner, the folder-link and file-request actions; `ShareLinkModal` gained folder support and targeted accounts/groups; `FileRequestModal` is new |
| E2E | **not added in this pass** — the backend matrix is covered by Rust integration tests; the browser flows (read-only banner, app-blocked tile, folder link, file request) still need Playwright specs |
| Docs | `docs/guides/explorer.md` (access matrix, app gate, folder/targeted links, file requests, explicit P3 scope), `docs/guides/shares.md` (two-layer model), `docs/API.md` (new fields/routes), `src/rag_indexers.rs` (Explorer FeatureDoc) |

Previous baseline: `tests/explorer_integration.rs` **99/99**,
`tests/share_links_integration.rs` **12/12**, `cargo test --lib` 2,675 passed.
Those figures are **not** re-stated as current until the added cases run.

## Phase 0 audit — 2026-10-31 (verified)

The Phase 0 inventory covers QW-05 through QW-11: backend modules and handlers,
frontend cards/routes, API entries, feature guides, migrations where applicable,
Rust unit/integration suites and Playwright specs. The implementation and the
acceptance matrix are verified below.

| Scope | Implementation evidence | Verification state | Next action |
|---|---|---|---|
| QW-05 Auto-block | `src/auto_block.rs`, middleware, handler, migration 065, Settings card | Verified: unit/integration/E2E green | No Phase 0 action remaining |
| QW-06 Connected clients | collector, handler, Settings card, API and Security guide | Verified: unit/integration/E2E green | No Phase 0 action remaining |
| QW-07 Share quotas | backend selection/enforcement, handler, migration 066, Shares UI and quota guide | Verified: unit/integration/E2E green; filesystem capability fallback covered | No Phase 0 action remaining |
| QW-08 Remote syslog | queue/framing worker, handler, Settings card and Journal guide | Verified: unit/integration/E2E green; mocked collector paths covered | No Phase 0 action remaining |
| QW-09 Wake-on-LAN | packet builder, host repository, handler, scheduled-task pre-step and card | Verified: unit/integration/E2E green; isolated listener path covered | No Phase 0 action remaining |
| QW-11 Configuration backup | archive codec, handler, migration-independent storage, card and Settings guide | Verified: unit/integration/E2E green on disposable data directory | No Phase 0 action remaining |

A focused contract test was added in `src/connected_clients.rs` to protect the
unknown-client refusal message. The handler must render `KickError::Unknown`
through its user-facing message rather than returning the bare client id.

**Phase 0 execution update — 2026-10-31:** backend, frontend and dedicated E2E
acceptance checks are verified. `cargo test --lib`: **1,984 passed,
0 failed**. Dedicated integration suites: `auto_block_integration` **14/14**,
`connected_clients_integration` **6/6**, `share_quota_integration` **5/5**,
`syslog_forward_integration` **11/11**, `wol_integration` **7/7** and
`config_backup_integration` **6/6**, totaling **49/49**. `cd frontend && npm run
check`: **0 errors, 0 warnings**.

> Both figures are the 2026-10-31 snapshot. The current baseline is
> **`cargo test --lib` 2,675 passed, 0 failed**, the six integration suites are
> still **49/49**, and `npm run check` is still 0/0. The E2E figures below are
> superseded by the P0.1 rerun section, which is the current one.

The six dedicated E2E specs were executed together on a rebuilt scratch Firefox
server with isolated `DATA_DIRECTORY` and one worker. Final result: **70 passed,
2 skipped, 0 failed** across 72 runtime tests. The failures were
classified before correction:

- confirmed product defects: TOML interpolation used `{ip}`/`{size}` while the
  frontend translator replaces `%ip%`/`%size%`; the launcher lacked
  `data-app-id` on normal app tiles;
- stale E2E contracts: Settings switches are native labelled checkboxes, while
  several specs still searched for `role="switch"`; Syslog expected `idle` for
  a disabled forwarder; an empty WoL host list was expected to be visible while
  the component rendered an empty hidden list;
- environment limitation: the scratch server had no passwordless sudo, so
  secondary-user creation and host service setup failed. Those access-control
  failures are not treated as product regressions until the suite runs in the
  documented privileged test environment.

The confirmed product defects and stale contracts were corrected in the source
and specs: i18n interpolation, card-scoped switch selectors, required member
email payloads, launcher fallback, Syslog observation invalidation, and quota
interpolation. The two skipped tests are conditional environment-state skips,
not failures. Phase 0 is verified for the QW-05 through QW-11 acceptance scope.

## B1 — Explorer file tags, 2026-11-03 (verified)

Per-user tags for Explorer paths, with a tag filter on the existing ACL-scoped
`GET /api/search/files` endpoint. Adding a tag never expands the roots the caller
can search, and deleting an account purges its tag rows.

| Coverage | Count / state |
|---|---|
| Pure validation | `src/storage/file_tag_repo.rs` **3/3** — trimming and case, invalid characters and path-shaped input, character (not byte) length bounds |
| Integration | `tests/explorer_integration.rs` **99/99** (1 tag test) — per-user isolation, scoped listing, invalid path/tag, idempotent duplicate, delete, **deleting a tag whose path is gone**, foreign-tag `NotFound`, missing-session guard |
| Search integration | `tests/global_search_integration.rs` **14/14** (1 tag test) — tag owner isolation, accessible-root confinement, malformed tag refusal, and `tag` echoed only when supplied |
| E2E | `tests/e2e/explorer/explorer.spec.ts` 15.5 — **1/1 green on Chromium and 1/1 on Firefox** (live server, scratch `DATA_DIRECTORY`, `e2eadmin` seeded through `POST /api/auth/setup`) |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |

Verified 2026-11-03 on a live server: sidebar add/click-through/remove through
the UI, the API's ACL refusal for an admin-invisible path, an outsider's tags
and writes being invisible, the tag filter through the ⌘K overlay, and the
tag-filtered global search.



### Two defects the verification found and fixed

1. **A tag on a deleted path could never be removed.** `DELETE /explorer/tags`
   required the path to still exist, so the sidebar × was a dead control the
   moment the file was deleted — the one case where a user most needs to clear
   a label. Removal is now ownership-scoped (a foreign tag is `NotFound`,
   disclosing nothing) and no longer touches the filesystem.
2. **The file-coverage note was invisible when it mattered.** In
   `SearchResults.svelte` the "2 locations did not answer" line was rendered
   inside the `results.length > 0` branch, so a partial answer with zero hits
   read as a clean "no results" — the exact failure E2E `desktop 6b.3` was
   written to catch (it was red on `main` before this change, now green).

## C1 — Local DNS server, 2026-11-05 (verified, no live apply)

The appliance's own resolver, riding on the **same `dnsmasq`** as the DHCP
server: local A/alias records, answered domains, upstream forwarders and a
switch that tells the LAN's DHCP clients to resolve here.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/local_dns.rs` **22/22** — name grammar (case, trailing dot, label rules) and every character that would change the meaning of a `dnsmasq.conf` line (space, tab, newline, `#`, `/`, `*`, doubled/leading dot, `_`, oversized label and name), domain validation keeping the subdomain-only leading dot, host-address rules (`0.0.0.0`, broadcast, multicast refused), the loopback forwarder refused **by name**, the three enable gates (interface, forwarder, confirmation), the lists still validated while off, `serve_dhcp_clients` refused while the server is off, one answer per name, an alias whose target this appliance does not serve, a domain entry colliding with a name record, duplicates, bounds (3 forwarders / 100 names / 100 domains), the interface-name ceiling, the rendered section (off emits nothing; on emits `no-resolv`/`domain-needed`/`bogus-priv`/`local-service` + one line per entry, with `interface=` left to the DHCP half), and `resolv.conf` parsing |
| Pure logic (composer) | `src/dhcp_server.rs` **15/15** (+5) — `render_conf` binds each serving interface exactly once (DHCP and DNS on the same interface ⇒ one `interface=` line, one `bind-interfaces`), carries the DNS section, drops the DHCP card's `dhcp-option=6` list when the appliance advertises itself, and `daemon_wanted` over the three switches |
| Integration | `tests/local_dns_integration.rs` **14/14** — `401` without a session and with an unknown token, the guard running **before** the body is parsed (a malformed body is still `401`, never `422`), a member allowed to read and refused the write (`403`), the GET payload (off by default, `available`, `interfaces`, `system_forwarders`, `state`), every `400` refusal (malformed body, no confirmation, no forwarder, directive injection in a name, loopback forwarder, alias target), and the document round-trip through the module's own helpers |
| Integration (guards) | `tests/phase4_auth_integration.rs` **6/6** — the two routes join the Phase 4 access-control matrix (no session, bogus session, member read, member write, admin never refused) |
| Integration (DHCP, unchanged) | `tests/dhcp_server_integration.rs` **21/21** against the recomposed drop-in |
| E2E | `tests/e2e/settings/local-dns.spec.ts` **12/12 on Firefox and 12/12 on Chromium** — missing-`dnsmasq` banner with no dead form, off by default with the body appearing only when on, the interface chooser offering the appliance's own interfaces, the forwarder/name/domain editors (add and remove, nothing saved until Save), the confirmation before enabling (nothing PUT before it, `confirmed: true` and the typed forwarder in the payload), cancelling saves nothing, a refusal surfacing the server's own sentence, the shared-daemon badge, keyboard toggling + labelled controls + an accessible name on every remove action, no value in the URL or in `localStorage`, and no horizontal overflow at 360/768/1920 px |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |
| RAG | `FeatureDoc` `local-dns` added; `src/rag_indexers.rs` guide-set test green (the guide set and `FEATURE_GUIDES` are asserted in both directions) |

**Nothing was applied to the host, deliberately.** A valid save would write
`/etc/dnsmasq.d/homevault.conf` and `systemctl restart dnsmasq` on the machine
running the suite, rewriting the test host's own resolver. Two things stand in
for the live round trip: the rendered file was syntax-checked against the real
**`dnsmasq` 2.91** (`dnsmasq --test --conf-file=…` accepts a rendered-shape file
and rejects a deliberately broken one), and every host call is bounded by a
deadline. A `dig nas.lan` round trip and a `systemctl restart` belong on a
disposable VM and are listed under "remaining to test live".

**`cargo test --lib`: 2,474 passed, 5 failed** — the same five pre-existing
failures `TESTING.md` has tracked since Phase A (`checksum::…evicts…`,
`handlers::settings::…appearance_no_longer_carries_theme_or_density`,
`i18n::…appearance_keys_are_at_parity…`, `sse::progress` ×2), none in a file this
change touches. Also re-run green: `explorer_integration` 99/99,
`global_search_integration` 14/14, `network_integration` 28/28,
`snmp_integration` 10/10. `tests/e2e/settings/{dhcp,snmp-agent}` stay green
(13/13 on Firefox).

## C2 — Generic reverse proxy, 2026-11-06 (verified, no live second device)

User-defined `host [+ path] → upstream` rules, served by **one** host-routed
listener (default port 80, `HOMEVAULT_REVERSE_PROXY_PORT` overrides it). The
rules live in `reverse_proxy_rules` (migration `076`); the sign-in gate is a
cross-host one-time ticket exchange, because the per-app `hv_app_session`
cookie is host-scoped and a generic rule deliberately uses another name. Design
note: `docs/analysis/c2-reverse-proxy-design.md`.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/reverse_proxy/mod.rs` **21/21** — host grammar (case, trailing dot, refused port/scheme/wildcard, IPv4 + bracketed IPv6), path prefix (root → whole host, refused `..`, `//`, query, fragment, backslash, bounds, reserved callback), upstream parsing (IPv4/name, bracketed IPv6, refused scheme/path/port 0/overflow), longest-segment rule selection (and `/api` ≠ `/apix`, disabled/other host ignored), the private-range matrix (loopback/private/link-local/ULA/IPv4-mapped accepted; public/unspecified refused), mixed answers refused, self-loop into the listener and into the dashboard refused |
| Pure logic | `src/reverse_proxy/proxy.rs` **6/6** — `next` stays a relative path (absolute/scheme-relative/control characters → `/`), both redirect builders (TLS-aware entry, stored-rule callback, percent-encoding), inbound `Forwarded`/`X-Forwarded-*`/`X-Real-IP`/`X-Rewrite-URL` discarded and replaced from the peer socket, both HomeVault cookies stripped while the service's own cookie survives |
| Pure logic | `src/reverse_proxy/ticket.rs` **4/4** — single use, rule binding, per-rule revocation, opaque unique tokens |
| Integration | `tests/reverse_proxy_integration.rs` **21/21** — `401` on every method without a session, member read vs admin-only mutations (`403`), guard before body parsing, full CRUD round-trip, duplicate `(host, path)` refused, unknown id `404`, every validation refusal, public upstream refused at save **and** at request time, reserved path refused, `/proxy-auth` without a session keeps `next`, unknown rule `404`, listener-down `503`; plus a **real listener** on an ephemeral port against a loopback upstream: forged headers replaced, custom header kept, Host rewritten, longest prefix wins, unknown host `404`, gated rule redirects with the encoded `next`, ticket redemption sets the rule-scoped cookie on the rule host, replay `401`, HomeVault cookies never reach the upstream |
| E2E | `tests/e2e/settings/reverse-proxy.spec.ts` **11/11 on Firefox and 11/11 on Chromium** — listener state, empty state, server-sentence validation, create, disable + edit, named delete confirmation, public-upstream refusal, keyboard open/Escape with focus restore, no horizontal overflow at 360/768/1920 px, no rule value in the URL or `localStorage`, no uncaught page error. Real backend; every created rule is removed in `afterAll` |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |
| RAG | `FeatureDoc` `reverse-proxy` added; the guide set and `FEATURE_GUIDES` are asserted in both directions |
| Live probe | On the scratch instance (`PORT=8311`, listener `8391`): an open rule served a real loopback HTTP upstream; a gated rule returned `302` to the server-chosen dashboard entry, `/proxy-auth` with a real `hv_session` minted a ticket, the callback set `hv_app_session` on the rule host and redirected to the original path, the authenticated request reached the upstream, and the ticket replayed as `401`. The forged-`Host` check is structural: the entry host came from the OS hostname, not from `Host: gated-proxy.test` |

**Never exercised live:** a browser on a second device resolving a rule name
through the C1 DNS server, and a TLS upstream with a publicly trusted
certificate (no self-signed pinning in this version). Both belong on a
disposable VM. The listener itself was exercised live on loopback above.

**`cargo test --lib`: 2,505 passed, 5 failed** — the same five pre-existing
failures (`checksum::…evicts…`, `handlers::settings::…appearance…`,
`i18n::…appearance_keys…`, `sse::progress` ×2), none in a file this change
touches.

## C3 — Router port forwarding (UPnP-IGD), 2026-11-06 (verified, no live router)

Lists the router's port mappings and external address through `upnpc`
(miniupnpc), adds one mapping and removes one again. Discovery is read-only;
every mutation is admin-only, confirmed, validated before `upnpc` runs, and
reversible from the same list.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/upnp.rs` **16/16** — `upnpc -l` parser (gateway, external IP, local address, description with spaces, permanent lease, malformed row ignored, no-gateway refusal), internal client must be private/link-local/ULA (public, loopback, unspecified, host name refused), ports `1..=65535`, lease `0` or `60..=604800`, description prefix/bounds and quote/backslash/newline/control refusals, exact `-a`/`-d` argv, confirmation on both mutations, default one-hour lease |
| Pure logic (handler) | `src/handlers/upnp.rs` **1/1** — the lease label (`permanent` / seconds) |
| Integration | `tests/upnp_integration.rs` **9/9** — `401` on every route, member read vs admin-only mutations (`403`), guard before body parsing, missing confirmation, public/loopback/host-name internal clients, zero ports, out-of-range lease, unknown protocol, malformed body, remove confirmation/port, and the `GET` shape (`available` is a boolean; an absent client is not an error) |
| E2E | `tests/e2e/settings/upnp.spec.ts` **9/9 on Firefox and 9/9 on Chromium** — missing-`upnpc` banner with no dead form, a discovery failure showing the router sentence, the mapping list with external address and a permanent lease label, the empty state, the add flow (default host, Save disabled until confirmation, exact payload), a refused add surfacing `ConflictInMappingEntry`, the named remove confirmation and its confirmed DELETE, keyboard open/Escape with focus restore, no horizontal overflow at 360/768/1920 px. Every backend call is intercepted, so no real router is touched |
| Capability | `dependencies.rs` spec for `upnpc` / `miniupnpc` (Network, optional) and `installer/install.sh` `install_optional miniupnpc`; the "every commanded binary has an install source" and English-only installer tests stay green |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |
| RAG | `FeatureDoc` `upnp` added; guide set asserted in both directions |

**`cargo test --lib`: 2,522 passed, 5 failed** — the same five pre-existing
failures (`checksum::…evicts…`, `handlers::settings::…appearance…`,
`i18n::…appearance_keys…`, `sse::progress` ×2), none in a file this change
touches.

**Never exercised live:** a real UPnP-IGD router. The machine hosting the suite
has no gateway, and opening a port on it would change that machine's network
face. The listing parser is tested against the `upnpc -l` shape; a manual pass
on a disposable LAN with UPnP enabled is the remaining check.

## D2 — Web Push backend, 2026-11-06 (verified, no live push service)

The missing backend half of the service worker's `push` handler: VAPID keys,
per-device subscriptions, RFC 8291 encryption and a sender in the notification
engine.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/web_push/crypto.rs` **10/10** — RFC 8291 Appendix A key schedule (CEK + nonce), ciphertext, 86-byte header and full body byte-for-byte; a generated `encrypt_push` record has the right shape/salt; VAPID JWT verified with the minted public key and its `aud`/`sub`/`exp` claims; bad key/secret lengths and an over-limit payload refused; base64url and sealed-PKCS#8 round trips |
| Pure logic | `src/web_push/mod.rs` **6/6** — endpoint must be HTTPS on 443 with no credentials/fragment; public-address policy (loopback/RFC1918/ULA/link-local/unspecified/multicast refused); payload bounded and UTF-8-safe; endpoint host redaction |
| Integration | `tests/web_push_integration.rs` **10/10** — `401` matrix, VAPID key surface (`available`, 87-char public key, empty list), guard before body parsing, non-HTTPS/other-port/credential endpoints, private/loopback endpoint literals refused before any connection, key-shape refusals, unknown device `404`, per-user isolation, re-subscribe upsert (same id, one row), account-deletion cascade |
| E2E | `tests/e2e/settings/web-push.spec.ts` **7/7 on Firefox and 7/7 on Chromium** — unavailable server half, insecure context, empty state, the enable flow with a scripted Push API (exact subscription payload, this-device badge), denied permission with no POST, named device removal with the DELETE, no overflow at 360/768/1920 px, no uncaught error |
| Live probe | On the scratch instance: `GET /api/notifications/push` generated the key pair, answered an 87-character public key, and wrote `data/web_push.json` sealed (`hv1:`) with mode **0600** |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |
| RAG | `FeatureDoc` `web-push` added; guide set asserted in both directions |

**`cargo test --lib`: 2,538 passed, 5 failed** — the same five pre-existing
failures (`checksum::…evicts…`, `handlers::settings::…appearance…`,
`i18n::…appearance_keys…`, `sse::progress` ×2), none in a file this change
touches.

**Never exercised live:** a real delivery through FCM/Mozilla/Apple. The suite
has no public push endpoint; the encryption is pinned by the RFC vector and the
delivery decision table is covered by unit/integration tests. A phone with the
PWA installed is the remaining check (and the only way to observe the service
worker's notification UI).

## D1 — Content index, 2026-11-06 (verified, live probe on a scratch share)

The last roadmap item: an opt-in, appliance-wide FTS5 index over the text files
of administrator-chosen Explorer roots, filtered per caller at query time.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/content_index/mod.rs` **14/14** — off-by-default safety, enabling needs confirmation + a root, absolute/normalised roots, root/retention bounds, exclusion normalisation, glob matching, text allowlist, hidden-name policy, safe quoted FTS expression (operators treated as text, 8-token cap), boundary-aware ACL prefix, candidate-root rule, hit cap |
| Pure logic (migration) | `src/storage/database.rs` +1 — `parse_sql_statements` keeps a `CREATE TRIGGER … BEGIN … ; … END;` body as one statement (migration 078 depends on it) |
| Integration | `tests/content_index_integration.rs` **9/9** — FTS insert/update/delete triggers (old term gone after an update), retention + per-root prune, admin gate on every write, default config/candidates, confirmation/root refusals, delete-my-index requires `confirm=true` and disables, search off until enabled, ACL filter (a `/etc` document is never returned to a caller rooted elsewhere), operator-only query returns empty |
| E2E | `tests/e2e/settings/content-index.spec.ts` **7/7 on Firefox and 7/7 on Chromium** — off by default with fields hidden, the named confirmation (and cancel), exact save payload (roots, exclusions, retention, `confirmed`), a refused save showing the server sentence, the status line and rebuild, the named delete with the document count, no overflow at 360/768/1920 px. Every backend call intercepted |
| Live probe | On the scratch instance with `HOMEVAULT_EXPLORER_ROOTS` pointing at a temp share: enabled + confirmed, the automatic run indexed **2** text files (a NUL-byte `.txt` was skipped), `q=watermelon` returned the matching path with its snippet and root kind, `q=banana` found the nested Markdown file, and `DELETE …?confirm=true` removed **2** documents and turned the feature off (`available: false` after) |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |
| RAG | `FeatureDoc` `content-index` added; guide set asserted in both directions |

**`cargo test --lib`: 2,553 passed at the time, 5 failed** — the five
long-standing failures. They were fixed on 2026-11-06; the suite is now
**2,558 passed / 0 failed** (see "Suite hygiene" above).

**Never exercised live:** a multi-terabyte share, a root that disappears
mid-run, and the 50 000-document cap. The per-file rules, the caps and the FTS
triggers are covered above; a large-share run belongs on a disposable VM.

## Suite hygiene — 2026-11-06 (suite green)

No feature item was left; the last work was removing the 12 tests that had
been red since Phase A. Two were product drift, one was a return-contract bug,
one was a real UI race, and the rest were stale test data.

| Fix | Where |
|---|---|
| `AppearanceSettings.density` removed from the live model, default and PUT validation; the eight dead `settings.density*` keys deleted from all four locales. The on-disk reader keeps the field only so an older `settings.json` still loads (and never writes it again). Both failing tests were right. | `handlers/settings.rs`, `settings_persistence.rs`, `src/i18n/*.toml` |
| `emit_progress` / `emit_message_only` / `emit_transfer_progress` now return "the job was updated", not "a subscriber received the broadcast" (a closed dashboard is normal). The speed test's datum moved from `t+11` to `t+12` so its `5 MB/s` expectation matches the arithmetic. | `sse/progress.rs` |
| The checksum cap test used one fixed `now` and increasing `recorded_at`, so the 15-minute TTL pruned 101 rows before the cap. One timestamp now isolates the cap; the expiry test still covers the boundary. | `checksum.rs` |
| **Real UI race:** opening the interface modal launched the config GET and showed the form immediately, so a late answer reset a radio the user had already changed. The modal now awaits the load before opening. The Diagnostics button also exposed no `network-btn` test id. | `frontend/src/routes/Network.svelte` |
| The seven network E2E cases now expect the shared guard's `401` for an absent token (instead of a stale `400`) and use the fixed modal/test id. | `tests/e2e/network/network.spec.ts` |
| The stale "content indexing is out of scope" and "push is missing" claims were removed from `ROADMAP-DSM.md`, `docs/guides/explorer.md` and the gap summary. | docs |

| Coverage | Result |
|---|---|
| Rust unit | `cargo test --lib` **2,558 passed, 0 failed** |
| Network E2E | `network.spec.ts` **46 passed, 4 conditionally skipped, 0 failed** on Firefox **and** Chromium (full suite, fresh scratch instance) |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |

The four skips are the honest hardware-conditioned cases (no WiFi hardware, no
bond to delete, no bridge); they are not failures and no longer hide a red
test.

## B4 — SNMP agent, 2026-11-04 (verified, no live daemon)

The appliance's own `snmpd`: generated read-only community, loopback or
private-LAN scope, `/etc/snmp/snmpd.conf.d/99-homevault-agent.conf`, settings
card and `GET|PUT /api/snmp/agent`.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/snmp_agent.rs` **20/20** — `public`/`private` refused by name, community bounds and characters (space, tab, newline, quote, `#`, control), private/link-local-only addresses (`0.0.0.0`, `127.0.0.1`, `8.8.8.8` refused), loopback scope discarding a stale address, generation + reuse of the community, the rendered config (read-only, no routing table, no wildcard, `-V systemonly`), the "no community answers nobody" guard, the redacted payload, directive injection through location/contact, and the v3 half: user-name alphabet, passphrase bounds and quote/backslash/control refusal, dedicated `createUser … SHA … AES …` + `rouser … authpriv systemonly`, generation + reuse of both passphrases, a half-configured user never rendered, the redacted payload carrying neither passphrase, sealed-at-rest round trip, the auth blob rejected as a privacy blob, and an old v2c document loading as `v2c` with the default user |
| Integration | `tests/snmp_integration.rs` **10/10** (+2) — the admin gate on both routes (401 without a session, 403 for a member) and the **capability contract**: `503` naming `snmpd` on read *and* write, with nothing persisted by the refused write |
| Sudoers | `src/setup.rs::test_snmp_agent_grants_are_scoped_to_its_own_file_and_unit` — the grant names our own file and the five `snmpd` systemctl actions, and fails if it widens to `snmpd.conf.d/*` or `systemctl *` |
| E2E | `tests/e2e/settings/snmp-agent.spec.ts` **7/7** — the missing-`snmpd` banner (and no dead form), off-by-default with the fields appearing only when on, the LAN scope offering only the appliance's own addresses, the save payload (an empty community is not round-tripped), a refusal surfacing the server's own sentence, the v2c → v3 swap (dedicated user + two passphrases, exact payload, no community), and an already-stored v3 passphrase not being round-tripped |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green |

**No live daemon was started.** Every E2E call is intercepted with
`page.route`, so the suite never writes `/etc/snmp/snmpd.conf.d/*` or restarts
`snmpd` on the machine hosting the run — the same rule the DHCP and fan-control
suites follow. What is *not* covered here is therefore `snmpwalk` against the
real agent: that needs a host with `snmpd` installed and root, and is listed
under "remaining to test live".

## B5 + B6 — Link aggregation and VLAN devices, 2026-11-04 (verified, no live apply)

Combine physical NICs into a bond (`802.3ad` or `active-backup`) and create
802.1Q VLAN devices, both configured through the existing apply / persist /
verify / roll back path.

| Coverage | Count / state |
|---|---|
| Pure logic | `src/network.rs` **71/71** (+9 for this change) — name bounds and characters, bond spec (mode, existing name, empty/duplicate member, non-NIC and virtual-"Ethernet" members), VLAN spec (derived name, id bounds 1/0/4095/4094, parent type, 15-char ceiling), the `ip` argv shapes, the netplan/ifupdown rendering, the mode round-trip, and the session-address refusal |
| Sudoers | `src/setup.rs::test_bond_and_vlan_commands_are_covered_by_the_ip_grant` maps every argv the module builds to a grant (with a small sudoers glob matcher); the existing per-object guard was tightened with an explicit "never netns/tunnel/rule" assertion |
| Integration | `tests/network_integration.rs` **28/28** (+4) — session vs admin split on all four routes, the two modes offered with the right `needs_switch_configuration`, an unsupported mode and an empty member list refused by name, VLAN ids 0/4095 refused with the range, deletion of a name no device carries refused |
| E2E | `tests/e2e/network/network.spec.ts` category 14 (+5) — **4/4 green + 1 skipped on Chromium**; the skip is 14.4 (delete asks first) because this host has no bond, which is the honest outcome for a delete test. The section, the LACP warning, both refusals and the labelled/keyboard-reachable controls pass |
| Frontend | `npm run check` **0 errors / 0 warnings**, `npm run build` green (the suite also dismisses the first-visit "What's new" dialog in `gotoDesktop`, which was blocking every launchpad click on a fresh server) |

**No live apply was run, deliberately.** Creating a real bond or VLAN rewrites
the networking of whatever machine runs the suite — the same reason
`network_integration.rs` has never exercised a real config change. Everything
up to the first `ip link add` is covered; the mutation itself and its
persistence need disposable hardware (or a VM with two spare NICs) and are
listed under "remaining to test live".

**Formerly pre-existing, fixed on 2026-11-06.** `network.spec.ts` 2.2, 4.3,
8.2, 10.1, 10.5, 13.1 and 13.2 are green now; see "Suite hygiene" below.

## B3 — SMS alerts, 2026-11-02 (verified)

Second Phase B item. One short message per alert, on the same events and
severity threshold as email and the webhook, through a **provider-agnostic**
HTTP POST — no vendor SDK, so a vendor changing its API cannot break the
appliance.

| Piece | Where |
|---|---|
| E.164 validation, one-segment composition, truncation, templating, request building (pure) | `src/sms.rs` |
| The only code that performs a POST | `notification_engine::dispatch_sms` |
| Persistence | `notifications.sms` in `data/settings.json` — no migration, an older file simply has no key |
| Settings form | `frontend/src/routes/Settings.svelte` (notifications category), 4 locales |
| Guide | `docs/guides/sms.md` (+ `INDEX.md`, `API.md`, `SCREENS.md`, one `FeatureDoc`) |

### Decisions pinned by tests

| Rule | Test |
|---|---|
| `+` then 8–15 digits; `0612345678`, spaced and punctuated forms refused | `international_numbers_are_accepted`, `plausible_but_undeliverable_numbers_are_refused` |
| The refusal teaches the expected shape | `the_refusal_names_the_expected_shape` |
| A disabled configuration is never refused | `a_disabled_configuration_is_never_refused` |
| Enabled needs a recipient, an endpoint and both placeholders | `enabling_without_*`, `a_template_without_the_placeholders_is_refused` |
| The token is never in any response | `the_token_is_never_returned_only_its_presence` (asserts on the serialized view) |
| `Authorization` comes only from the token, never from the admin's headers | `the_authorization_header_is_built_from_the_token_only`, `a_blank_token_produces_no_header` |
| One segment, marker on truncation, never split mid-character | `a_long_message_is_truncated_with_a_visible_marker`, `truncation_never_splits_a_multibyte_character`, `compose_text_uses_characters_not_bytes` |
| Refusal happens before any network call | `build_request_refuses_before_any_network_call` |
| A blank token keeps the stored one (HTTP round trip) | `test_saving_without_a_token_keeps_the_stored_one` |

### Verification

| Suite | Result |
|---|---|
| `cargo test --lib sms` | **19/19** |
| `tests/sms_integration.rs` | **10/10** |
| `cargo test --lib notification_engine` | **2/2** |
| `cargo test --lib settings_persistence` | **15/15** (the new field round-trips) |
| `cargo test --lib rag_indexers` | **5/5** (guide ↔ `FeatureDoc` both ways) |
| `npm run check` / `npm run build` | 0 errors, 0 warnings / green |

### Verified live, on a fresh instance

| Check | Observed |
|---|---|
| Save with a token | `200` |
| Read back | `{ …, token_set: true }` — no field that could carry the credential |
| On disk | `settings.json` holds `hv1:…`; the plaintext appears nowhere |
| A number without a country code | `400` — `'0612345678' is not a phone number in international form (expected a leading '+' and 8–15 digits, e.g. +33612345678)` |

### Deviation

**No automated delivery.** Sending for real would make the suite depend on an
external gateway, so the exact request (endpoint, body, headers) is asserted by
`build_request` instead. The guide says so rather than implying coverage.

## B2 — Read-only ISO/IMG mounting, 2026-11-02 (verified)

First Phase B item. An image is attached to a loop device read-only and appears
in the Explorer as one more folder, so a 6 GB ISO opens instantly instead of
becoming 6 GB of files on the array.

| Piece | Where |
|---|---|
| Validation, slug, containment, argv, `/proc/mounts` parsing (pure) | `src/iso_mount.rs` |
| HTTP surface + the only code that shells out | `src/handlers/iso_mount.rs` |
| Routes | `GET` + `POST /api/explorer/iso`, `DELETE /api/explorer/iso/{id}` in `src/web/mod.rs` |
| Sidebar panel | `frontend/src/lib/components/explorer/IsoMountPanel.svelte` |
| Guide | `docs/guides/iso-mount.md` (+ `INDEX.md`, `API.md`, `SCREENS.md`, one `FeatureDoc`) |

### Decisions pinned by tests

| Rule | Test |
|---|---|
| `.iso`/`.img` only, refusal names the accepted set | `supported_extensions_are_iso_and_img_only`, `unsupported_type_names_the_allowed_set` |
| The mount point is derived and deterministic | `slug_is_deterministic_and_readable`, `the_mount_point_is_derived_from_the_slug_not_the_request` |
| Two same-named images do not collide | `same_file_name_in_two_folders_gets_two_slugs` |
| Containment, including against `..` and prefix siblings | `mount_point_must_stay_under_the_iso_root` |
| `ro,nosuid,nodev` on every mount; no `--detach-all` | `mount_is_read_only_nosuid_and_nodev`, `detach_names_one_device_only` |
| `/proc/mounts` is the authority | `only_mount_points_the_host_confirms_are_reported`, `a_mount_outside_the_isolated_root_is_ignored` |
| Nothing is mounted twice | `the_same_image_is_not_mounted_twice` |
| The guard runs before the body | `test_a_malformed_body_without_a_session_is_still_401` |
| A crafted id cannot name a path | `test_a_traversal_in_the_id_cannot_name_a_path` |

### Verification

| Suite | Result |
|---|---|
| `cargo test --lib iso_mount` | **25/25** (20 module + 5 handlers) |
| `tests/iso_mount_integration.rs` | **11/11** |
| `cargo test --lib rag_indexers` | **5/5** (the guide ↔ `FeatureDoc` mapping holds both ways) |
| `cargo test --lib utils::process::…::every_subprocess_of_the_tree_is_bounded` | green — the new `sudo losetup`/`mount`/`umount` all carry a named deadline |
| `cargo test --lib dependencies::…::test_every_commanded_binary_has_an_install_source` | green — `losetup`/`mount`/`umount` ship in `util-linux`, already in `install.sh`'s required group |
| `cargo test --lib` | **2,404 passed** (+25); the same 5 pre-existing unrelated failures |
| `npm run check` / `npm run build` | 0 errors, 0 warnings / green |
| `cargo clippy --lib`, `cargo fmt --check` on the new files | clean |

### Deviation

**No automated successful-mount round trip.** The build host has no
passwordless sudo and a real `losetup`+`mount` would leave a device attached to
the machine running the tests, so the argv, the containment and every
pre-syscall refusal are asserted directly instead. This is the same class of
deviation `docs/guides/iscsi.md` records for its hardware acceptance suite, and
the guide says so rather than implying coverage.

## P0.2 — Capability and API contracts closed, 2026-11-02 (verified)

The API had a documented auth contract (`admin_guard`, stated once) but no
equivalent for the other cross-cutting answers. The gap was **status drift**:
a missing external tool was answered `400` (`json_error`) — the status reserved
for a request the server could not understand — at 38 call sites.

Measured before changing anything:

| Module | `400` sites for a missing tool |
|---|---|
| `handlers/docker.rs` | 26 (`Docker is not available`) |
| `handlers/restic.rs` | 3 (`restic CLI is not installed`) |
| `handlers/firewall.rs` | 1 (`ufw is not installed`) |
| `handlers/antivirus.rs` | 1 (`ClamAV (clamscan) is not installed`) |
| `handlers/backup_target.rs` | 1 (`restic is not installed on this host`) |
| **Total** | **32 call sites** (38 `json_unavailable` occurrences after expansion) |

### What was added

| Piece | Where |
|---|---|
| The contract itself | `docs/API.md` → Conventions: capability/unsupported, admin mutations + audit + `requires_restart`, secrets |
| `ResponseHelper::json_unavailable(missing)` → **`503`** | `src/utils/response.rs` |
| `ResponseHelper::json_not_implemented(message)` → **`501`** | `src/utils/response.rs` |
| The 32 sites converted | `docker`, `restic`, `firewall`, `antivirus`, `backup_target` |
| A guard that keeps the three statuses apart | `src/utils/response.rs` (`#[cfg(test)]`) |

The helpers take `impl Display`, so a formatted sentence works the same as a
literal — the `'7z' is not installed on this server` shape is covered by a test.

### Rules now pinned in writing

- **`503` = this host cannot; `501` = this build cannot; `400` = your request is
  wrong; `401`/`403` = identity.** Four different questions, four different
  statuses; no caller has to parse English to tell them apart.
- **Capability discovery is a `GET`.** The established field names are
  `available: bool`, `capabilities: {…}`, `supports_<thing>: bool` and
  `<thing>_available: bool`, and a screen that shows a `supports_*: false` must
  not offer the action either (traffic control's `supports_download_limit` is
  the reference).
- **A failure must name the missing piece**, so the UI can say what to install
  instead of a generic toast.
- **The guard runs before the body is parsed** (Phase A made it true; this pins
  it as the rule, because `Json<T>` first leaks the schema with a `422`).
- **A privileged action is journaled** naming the account and the target, never
  a secret; a refused save keeps the previous state.
- **A secret is never returned**, sealed at rest with its scope as AAD,
  travelled in a header never a URL, and "shown once" is announced before the
  user dismisses it.

### Verification

| Check | Result |
|---|---|
| `cargo test --lib utils::response` | **4/4** — `503` + message, formatted sentence, `501`, statuses distinct |
| `cargo test --lib docker` | **23/23** |
| `tests/docker_integration.rs` | **10/10** |
| `tests/restic_integration.rs` | **26/26** |
| `tests/backup_target_integration.rs` | **8/8** |
| `cargo clippy --lib` on the six touched files | clean |
| `cargo fmt --check` on the six touched files | clean |
| `grep 'json_error("… not available\|not installed")'` | **0 remaining** |

No test asserted the old `400` for these messages, so the conversion changed no
expectation — only the status a real client sees.

### Not converted, and why

A tool name carried inside an internal `Err(String)` — the archive extractor
naming a missing `7z`, `vm.rs` naming `ovmf` — keeps the status of the operation
that failed. The contract governs the **capability gate**, where a handler
decides up front that it cannot proceed; converting an internal error string
into a status would mean re-deriving a status from a message, which is the
antipattern `admin_guard`'s `Denial` enum exists to avoid.

## P0.1 — Quick-win E2E verification pass closed, 2026-11-02 (verified)

`ROADMAP-DSM.md` P0.1 was open since 2026-10-31: "privileged rerun pending",
with a first pass of **39 passed / 32 failed / 1 skipped** and a set of failures
classified as an *environment limitation* because the scratch host had no
passwordless sudo. The exit criterion was that every claimed status
distinguishes **implemented**, **integration verified** and **E2E verified**, and
that no feature is green from compilation alone.

**The rerun is done, on both browsers.** The six dedicated suites are
`autoblock`, `connected-clients`, `syslog`, `wol`, `config-backup` and
`shares/quota`.

| Scope | Firefox | Chromium |
|---|---|---|
| 6 dedicated E2E suites | **70 passed, 2 skipped, 0 failed** | **70 passed, 2 skipped, 0 failed** |
| Runtime | 4.6 min | 3.5 min |

Run against a rebuilt debug binary, isolated `DATA_DIRECTORY`, `--workers=1`,
one freshly seeded admin. The **32 failures of the first pass are all gone**;
the count now matches the "after correction" figure recorded on 2026-10-31
(70/2/0), and this time it is reproduced on a second browser rather than taken
from a single run.

### The two skips are state guards, not failures

Both are `test.skip(condition, reason)` on a property of the instance, so they
pass or skip depending on what the server already contains — never on a defect:

| Test | Guard |
|---|---|
| `autoblock.spec.ts` 3.3 — a failed pardon is shown as an alert | `state.bans.length === 0` → "no blocked address to pardon" |
| `syslog.spec.ts` 5.3 — the test button is disabled while forwarding is off | `settings.enabled` → "forwarding is enabled on this instance" |

Every other `test.skip` in the six files is the same shape (no share to quota,
no saved WoL host, no missing collector, forwarding already on, live bans
present). None of them hides an assertion behind an environment failure.

### The "environment limitation" no longer applies to these suites

The 2026-10-31 note said secondary-user creation and host service setup failed
because the scratch server had no passwordless sudo, and those failures were not
treated as product regressions pending a privileged environment.

Confirmed on this host: `sudo` **does** require a password here
(`sudo -n true` → "a terminal is required"). What that breaks, and what it does
not, was measured directly instead of assumed:

- `POST /api/groups` fails cleanly with `sudo: il est nécessaire de saisir un
  mot de passe` — a real limitation, reported to the caller, not swallowed.
- `GroupService::ensure_linux_user` is only reached when a **group is added**;
  creating a user is a SQLite row and needs no privilege at all.
- **None of the six suites touches a privileged path**: grepping them for
  `/api/groups`, `home_enabled`, `delete_home`, `getent`, `uid` and
  `/etc/passwd` returns nothing.

So the sudo limitation is real but **out of scope for these six suites**. Where
it still bites — group creation, Linux account provisioning, host service setup
— the failure is explicit and carries the command's own stderr; it is recorded
here rather than left as an unclassified "environment" bucket.

### Backend half, re-confirmed

| Suite | Result |
|---|---|
| `auto_block_integration` | **14/14** |
| `connected_clients_integration` | **6/6** |
| `share_quota_integration` | **5/5** |
| `syslog_forward_integration` | **11/11** |
| `wol_integration` | **7/7** |
| `config_backup_integration` | **6/6** |
| **Total** | **49/49** |

Identical to the 2026-10-31 measurement — the QW-05…QW-11 backend surface did
not regress while Phase A and Phase 4 were added.

### Status

P0.1 is **closed**. Every QW-05…QW-11 feature can now be claimed as
*E2E verified* on Firefox **and** Chromium, which is what the exit criterion
required. P0.2 (cross-cutting capability/API contracts) remains open and is the
only item left in Phase 0.

## Phase A — DSM-parity debt closed, 2026-11-02 (verified)

The five Phase 4 handler modules (`dhcp_server`, `usb_copy`, `fan_control`,
`ups_server`, `guided_redundancy`) shipped with **no session check at all** and
there is no global auth layer under `/api`, so all 22 of their routes were
reachable from the LAN without a session — including
`POST /power/ups-server/shutdown` (which runs `sudo poweroff`) and
`POST /network/dhcp-server/start` (which runs `sudo systemctl start dnsmasq`).
Phase A closes that, fixes three correctness defects in the same modules, and
brings them to the project's definition of done.

| Item | What changed | Verification |
|---|---|---|
| **A1** Auth on 22 routes | `require_auth` on every read, `require_admin` on every mutation; the guard runs **before** the body is deserialized, so an unauthenticated caller gets `401` with the standard envelope instead of a `422` that echoes the schema | `tests/phase4_auth_integration.rs` **6/6** |
| **A2** Correctness fixes | `docker stop "$(docker ps -q)"` passed as argv (stopped nothing, exited 0) → two-step `docker ps -q` + `docker stop`; `shutdown_vms`/`flush_restic` were `Ok(())` stubs → `StageOutcome::Skipped(reason)` with a new `skipped` field on the HTTP payload; `guided-redundancy/apply` documented as `501` and admin-only | `src/ups_server.rs` +5 unit; `src/handlers/ups_server.rs` +1 unit |
| **A3** i18n + design tokens | 5 components rewritten: 0 hardcoded literals, ~100 keys added to `en`/`fr`/`de`/`es`, `alert()`/`confirm()` replaced, hand-rolled overlays replaced by the focus-trapping `Modal`, CSS variables that did not exist (`--card-bg`, `--primary-color`) replaced by real tokens, click-only cards replaced by real radios | `npm run check` **0 errors, 0 warnings** (was 5 errors / 15 warnings); `npm run build` green |
| **A4** Documentation | 5 guides (`dhcp.md`, `usb-copy.md`, `fan-control.md`, `ups-server.md`, `guided-redundancy.md`), 22 endpoints in `API.md`, 5 screens in `SCREENS.md`, 5 rows in `INDEX.md`, 5 `FeatureDoc`s + the `rag.md` row | `rag_indexers` **5/5** — the guide set and the `FEATURE_GUIDES` map are asserted in both directions |
| **A5** E2E | 5 new specs under `tests/e2e/settings/` | **38/38 executed green on Firefox and 38/38 on Chromium** (2026-11-02, live server on port 8099) |
| **A6** Deadline discipline | 23 unbounded `Command::new` replaced by `utils::process::run_timed_async*` with named constants (`PROBE_TIMEOUT`, `CONFIG_WRITE_TIMEOUT`, `POOL_PROBE_TIMEOUT`, `DEVICE_OP_TIMEOUT`) | `utils::process::tests::every_subprocess_of_the_tree_is_bounded` green |

### Guards that were red on `main` and are green now

These were **pre-existing** failures, not introduced by Phase A. Three of them
proved that `main` could not even compile its lib test profile.

| Guard | Why it was red | Fix |
|---|---|---|
| `utils::process::tests::every_subprocess_of_the_tree_is_bounded` | 23 Phase 4 call sites had no deadline | A6 |
| `dependencies::tests::test_every_commanded_binary_has_an_install_source` | `dnsmasq`, `udisksctl`, `upsd` were executed but installable by nothing | three `DependencySpec`s + `install_optional dnsmasq` in `install.sh` |
| `fan_control.rs` compile | `MIN_SAFE_PWM * 100 / 255` overflows a `u8` at compile time (constant eval) | `MIN_SAFE_DUTY_PERCENT` computed in `u32`; same fix in `tests/fan_control_integration.rs` |
| `handlers::ups_server::tests` (2) | two tests raced on the process-global `DATA_DIRECTORY` | `ENV_LOCK` guard, matching `src/health.rs` |
| `ups_server_integration::test_validate_config_rejects_reserved_ports` | the code refused 22/80/443 but not 8443, which is HomeVault's own HTTPS listener | `matches!` covering all five |
| `guided_redundancy_integration::test_raid5_scores_higher_than_raid0` | compared two layouts that are never offered together (RAID 0 below 3 disks, RAID 5 from 3 up) | compares the two layouts three disks actually produce |

### Suites

| Suite | Result |
|---|---|
| `cargo test --lib` | **2,675 passed, 0 failed** (2026-11-07 baseline) |
| `tests/phase4_auth_integration.rs` | **6/6** |
| `tests/dhcp_server_integration.rs` | **21/21** |
| `tests/usb_copy_integration.rs` | **18/18** |
| `tests/fan_control_integration.rs` | **17/17** (did not compile on `main`) |
| `tests/ups_server_integration.rs` | **13/13** |
| `tests/guided_redundancy_integration.rs` | **19/19** |
| `tests/e2e/settings/{dhcp,fan-control,ups-server,usb-copy,guided-redundancy}.spec.ts` | **38 passed on Firefox, 38 passed on Chromium** (live server, `--workers=1`) |

### Two defects the E2E pass caught that no Rust test could

Both were invisible to `cargo test`, `npm run check` and `npm run build`, and
both made the five screens **dead in production**.

| Defect | Symptom | Cause | Fix |
|---|---|---|---|
| The server refused to boot | `thread 'main' panicked at src/web/mod.rs:1513: Path segments must not start with ':'` — the process exited before listening | the four Phase 4 routes were registered with Axum 0.7 path syntax (`:id`) on an Axum 0.8 router | `{id}` / `{profile_id}`, matching every other route in the file |
| Every Phase 4 screen was empty | the card rendered, the list/table stayed blank, no error | `api-phase4.ts` fetched with `Authorization: Bearer <localStorage.auth_token>` (a key nothing writes — the app stores `session-token`) **and** returned the raw `{ success, data }` envelope while callers read `response.config` / `response.channels` | rewritten on the shared `api` client, with an `unwrap()` that throws the server's own sentence instead of returning `undefined` fields |

The first one is the reason this section exists: **`cargo test --lib` and
`cargo check` were both green while the binary could not serve a single
request.** Only starting the real server surfaced it.

Also fixed while verifying live: `UsbCopy.svelte` rendered a second `<h1>`
inside the window chrome's own `<h1>` (now an `<h2>`), and
`storage.guided_redundancy` — the label of the "Suggest a RAID layout"
button — was missing from all four locales.

### Deliberately out of Phase A scope

Five lib tests remain red and none of them belong to the five Phase 4 modules:
`checksum::tests::test_outcome_store_evicts_the_oldest_at_the_cap`,
`handlers::settings::tests::test_appearance_no_longer_carries_theme_or_density`,
`i18n::tests::test_appearance_keys_are_at_parity_and_the_removed_one_is_gone`
(both part of an in-flight, untracked density feature) and two
`sse::progress` transfer-speed tests. They are tracked separately rather than
folded into a parity change.

Cross-references the 37 screens listed in `SCREENS.md` with three distinct
levels of testing, to answer "what is really tested, what remains to be
tested":

1. **Manual live audit** — real backend, real session, often
   Playwright driven by hand. Documented in dated prose/changelog in
   `CLAUDE.md` and `docs/guides/` (each documented invariant = one audit of
   this type). It is a
   one-off audit, not a guarantee against future regression.
2. **Automated Rust tests** — unit (`#[cfg(test)]` in the source
   file, pure logic) and integration (`tests/*.rs`, in-memory SQLite DB +
   `tower::ServiceExt::oneshot`).
3. **Playwright E2E tests** — `tests/e2e/<screen>/<screen>.spec.ts`, against
   a real server, no mocks.

Only levels 2 and 3 protect against future regression; level 1 is a snapshot
at a given date. Compiled on 2026-09-05, updated
on 2026-09-06 (addition of the **Network**, **VPN**,
**Shares/Protocols**, **Security suite**, **Downloader**, **Storage**,
**Backup**, **Cloud Sync**, **Syncthing**, **Chat**, **Explorer**
suites and the **Terminal** Rust tests), then on **2026-09-07** (filling the
gaps in the already partially tested screens: new
`tests/raid_integration.rs` (auth guards); `storage_integration.rs`
extended to the `virtual_disk_*` handlers; `explorer_integration.rs` extended
to `/roots` `/save` `/eject` `/snapshots` favorites and `?strategy=`; unit
tests `transfer_quota.rs::check_quota`, `handlers/storage.rs::vd_name`,
`browser.rs` collision-strategy, `handlers/explorer.rs`
`kind_for_root`/`resolve_dest`), by directly reading `CLAUDE.md`,
`tests/*.rs` and `tests/e2e/**` (real counting of the files and of the number
of `test(...)`/`#[test]`, not an estimate).

**2026-09-07 (real "host conditions" test — 4 areas, 10 bugs
fixed).** On explicit request, the 4 areas whose automated tests
stopped at "clean rejection" for lack of `sudo`/hardware access
— **Firewall (ufw)**, **Shares/SFTP**, **RAID/Storage (virtual disks)**
and **System journal** — were exercised end to end against the real
host (temporary NOPASSWD `sudoers` drop-in, 6 loop devices, a real
`ufw enable`/`disable` cycle with port 22 protected, a real
`homevault-sftp` `sshd`). **10 bugs found and fixed in frozen files**
(explicit authorization), plus the associated data-quality
fixes in `PoolService::scan_pools` (device_count / mount
point / mdadm size / ZFS mount point) — see the section
"## Live real test 2026-09-07" at the end of the file. Each fix was
re-verified live on the rebuilt binary.

**2026-09-07 (2nd pass "implement what remains" — same NOPASSWD
drop-in).** **de/es** i18n fixed (accents, values only);
**Rescue** exercised live for the 1st time (6 frozen bugs); **Storage/
LUKS** complete cycle live (2 frozen bugs); **Network**: reads
re-verified + application path `src/network.rs` fixed (`sudo` on
the `ip`/`dhclient` mutations, `resolv.conf` via `sudo tee` without clobber)
and verified live on a disposable `dummy` interface. Detail in the section
"## 2026-09-07 (continued)" and the dated addenda of `CLAUDE.md`. Tests:
rescue unit 8/8, rescue_integration 28/28, storage_integration 24/24,
network unit +3/3, network_integration 23/23.

**2026-09-13 (transfers v2).** Hardening of the Explorer transfer
methods and of share links: HTTP `Range` (206) + `X-Content-Sha256`
digest on downloads, short-lived tickets (single-use download,
multi-use thumbnail per directory) replacing the `?token=` in the
URLs, resumable chunked upload (session → chunks → complete), quotas
locked per-subject (reservation + refund, anti-race), canonical
`PathGuard` path in all handlers (TOCTOU symlink), `rename`
validates the destination, `/explorer/delete` admin-only, trash via
`copy_file_verified`, directory `fsync` after each atomic rename,
cleanup of the partial destination of a failed folder copy/move,
per-token rate-limit on `/share/{token}` + token without modulo bias. Detail
in `docs/guides/explorer.md` and `docs/guides/transfer-quotas.md`. Tests:
`explorer_integration.rs` 49/49, `share_links_integration.rs` 12/12,
unit `media_ticket.rs` 5/5, `handlers/explorer.rs` 17/17,
`browser.rs` 11/11, lib 592/592, `svelte-check` 0 errors. Explorer E2E to
re-run on a fresh server.

**2026-09-14 (desktop chrome — fidelity to the approved gateway mockup).** Dock and
topbar realigned to the reference mockup: each pill now has
**its own** height token (`--hv-chrome-dock-h: 84px` unlocked from
`--hv-chrome-topbar-h: 62px`), dedicated chrome material
(`--hv-chrome-surface-border` / `-shadow`, shared by the two pills and
their panels), dock tiles at 48px with a hover bounce and tooltip
(`.hv-dock-item::before`, on `--hv-tooltip-bg`), app gradients re-harmonized
on the Tailwind 500→700 scale with `--hv-app-<id>-c` = dark stop of the gradient.
No behavior change. Tests: `desktop.spec.ts` goes to
**51 tests / 14 categories** — 8.3 rewritten (the two heights are verified
separately, and "the dock is the tallest" only holds above the phone width),
new category 14 "Chrome geometry & material" (JS mirror `snapUtils.ts`,
badge = dark stop, opaque pills, tooltip). 1.5 finally covers
`dock-terminal`, which was rendered but never asserted. Count verified via
`npx playwright test --list --project=firefox` — the `grep -c 'test('`
method documented below over-counted (it caught the `.test(x)` of the
assertions); it is fixed to `grep -cE '^\s*test\('`.

**2026-09-14 (topbar buttons — icon + label chips).** The topbar adopts the
layout of the approved mockup (node `1:24`): the 4 buttons go from icon
only to **icon + label** from 901px, icon only below that (the target
stays 44×44 in both cases — only the width changes). Mockup order:
Network, Widgets, Notifications, Power. A decision formerly refused
(`docs/guides/desktop.md`) on three grounds, two of which were outdated: no
new i18n key is needed (the 4 labels reuse existing keys,
including `desktop.power`, until now orphaned — the only i18n edit: `es.power`
goes from "Apagar" to "Energía" to align with fr/de), and the 44px comes from
the mockup itself. **No token or keep-out moves** — the delta is
purely horizontal, the pill height (62px) still carried by the 48px user
chip. The notification badge is re-scaled to **16px**
anchored to the button (it previously shared a rule with `.hv-dock-badge`,
which keeps its 26px for its 48px tile). `desktop.spec.ts` goes from
**51 tests / 14 categories to 58 / 15**: category 15 "Topbar icon + label
chips" (7 tests — chip geometry, fallback ≤900px without a second markup
branch, DOM order, 16px badge vs dock 26px + non-overlap of the label,
WCAG 2.5.3, 901px in German), helper `topbarBtn`, and the 3 lines 7.1-7.3 that
targeted power via `.hv-topbar-btn').last()` rewritten as
`[data-testid="topbar-power"]`. `8.3`, `14.1`, `14.3` and `14.4` stay green
**without modification** — that is the proof of zero vertical ripple. Count
verified via `npx playwright test --list --project=firefox` (58) and
`grep -c '^test.describe('` (15).

**2026-09-14 (app window chrome — "AppWindow" mockup).** The app window is
realigned to the Figma mockup "Untitled" (node `3:76`),
measured to the pixel on its 2× export. The frame becomes **opaque
paper** (`--hv-window-bg` #FFFFFF, no more transparency), in a **slate-300**
hairline `--hv-window-border` (#CBD5E1 — deliberately darker than
`--hv-shell-border`, otherwise the frame dissolves into the wallpaper) and
the shadow specific to the mockup (`--hv-window-shadow`, two layers). The
header bar goes to a **constant height of 44px** (`--hv-winhdr-h` =
8 + 28 + 8), the icon tile to **24px / radius 8** (`--hv-winhdr-icon-*`,
app gradient kept), the title to **14px / 600 on 20px** in `#314158`
(`--hv-winhdr-title`). The min/max/close trio leaves its floating 48px
position for the bar itself: **28px**, radius 8, gap 6, at the exact stops
of the mockup `--hv-winctrl-min` #FFB900 / `--hv-winctrl-max` #2B7FFF /
`--hv-winctrl-close` #FF6467 — values **deliberately more vivid** than
the `--hv-warning`/`--hv-error` semantics, not to be "harmonized". The
glyphs are those of the mockup (14×14, `stroke-width: 1.458`, the value of the
SVG). Corollary: `WindowHeader` reserves `--hv-winhdr-btn-clear` **120px**
(3×28 + 2×6 + 12 inset + 12 gap; 108px at ≤480px, where the bar wraps),
and each button carries a transparent **34px** `::after` — the
maximum the 6px gap allows without two neighboring targets
overlapping — to stay usable by touch despite the 28px of the mockup.
No `data-testid` moves, so categories 3 and 15 stay green as
they are. `desktop.spec.ts` goes from **58 tests / 15 categories to 63 / 15**:
category 14 completed by 5 fidelity-to-the-mockup tests (14.5 opaque surface
+ slate-300 hairline + two-layer shadow + `backdrop-filter:none`;
14.6 44px bar, 24px/radius 8/gradient tile, 14px/600/#314158 title on
20px; 14.7 28×28/radius 8/gap 6 trio with measured inter-button gap and
12px inset in the bar — not overlaid; 14.8 the reserve really protects
the title, **measured** at 1440/768/400px rather than read in the
tokens, because the trio is absolutely positioned: the right padding of the
bar is the only thing preventing it from going underneath; 14.9 **a single
header per window**, on a sample of the registry). Three method points:
14.6 asserts the `min-height` of 44px and a render **≥** 44, not 44
hardcoded — **all** the shipped apps pass a `subtitle`, so their bar
is taller than the mockup (which is title-only); the bottom rule is
measured on `.hv-winhdr-wrap`, **not** on `.hv-winhdr` — the rule is carried
by the container (which also includes the optional tab row), so reading
`borderBottomColor` of the bar returns the inherited `currentColor` (the ink
`#1F2733`) and can never match — the first version of 14.6 failed
exactly on that; and 14.9 was written
**after finding a real bug**: `quotas`, added on 2026-09-14, was missing
from the `OWN_HEADER_APPS` `Set` of `WindowFrame.svelte`, so the Quotas
window stacked **two** bars (its own + the WindowFrame fallback, whose
sprite name has no glyph → empty bar). Fixed by adding
`'quotas'` to the `Set` + a comment that points to 14.9. It is the only app of the
registry that was missing; no `data-testid` or token is concerned. Count
verified via `npx playwright test --list --project=firefox` (63) and
`grep -cE '^\s*test\.describe\('` (15).

**2026-09-14 (standardization of window headers + dock tile gap).**
`WindowHeader` is now **icon + title only**: the `subtitle` prop is removed
from `WindowHeader.svelte` and from `PageLayout.svelte`, and
the 22 routes that passed one no longer do. The icon tile is worth
`--hv-winhdr-icon-size` = `--hv-winctrl-size` (28px, the size of the min/max/close
trio) and the three are centered on the same line; `.hv-dock-container`
goes to `--hv-chrome-dock-gap` (10px) because on hover a tile grows by
4.3px on each side and the old `gap` of 4px made them overlap.
Nine specs were realigned (desktop, account, health, journal, rescue,
dashboard, users, network, groups, shares): the subtitle
assertions become `toHaveCount(0)`, the removed decorative badges/dots are
removed from the expectations, one test removed with its object (account 1.3), one added
(desktop 14.10) and one rewritten (rescue 1.3, availability moves from the
header band to the tool's card). **None of these specs was re-run and nothing
was compiled** — the session instruction explicitly forbade it; the
per-screen counts of the next section were therefore produced by
`grep -cE '^\s*test\('` (the file's method) and not by a run. The lines
that announced an outdated number (account 76→72, groups 61→60, dashboard
39→37, desktop 63→64) are fixed.

**2026-09-16 — Terminal: the tab strip leaves the window header.**
`terminal-tabs` was rendered in the `center` snippet of `WindowHeader` — so
in the title bar, competing with the min/max/close trio pinned to its
right edge. It is now a **window content bar**, direct child
of the `terminal-page` container, between the header and `terminal-infobar`:
`WindowHeader` → `terminal-tabs` → `terminal-infobar` → xterm body. Immediate
corollary: the Terminal header passed **no** `title` (the tabs occupied the
space), it would no longer display anything but its icon tile — it therefore receives
`title={$t('terminal.title')}` (key already present in the 4 locales).
Geometry measured live: header 44px, strip 44px, infobar 32px, the strip
not being a descendant of `.hv-winhdr`. New test **2.8**, and four
tests repaired — **already red before this change**, selectors outdated since
the Tailwind-on-tokens migration of 2026-09-12: 3.2 (`page.locator('hv-tab-active')`,
a *tag* selector — it was missing the dot of `.hv-tab-active`) and 3.4 /
7.3 / 7.4 (they expected the semantic classes `hv-tab-dot` / `hv-info-badge`,
removed in favor of tokens; they now assert the **computed fill**, which is the real
state marker). Verified by a real run (Firefox,
fresh disposable instance on a scratch port, dedicated `BASE_URL`, no real
data): sections 1, 2, 3, 4, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18
green, `svelte-check` at 0 errors. Four failures remain, **all proven
pre-existing** by `git stash push -- frontend/src/routes/Terminal.svelte`
(the file restored to its state before, they fail identically):
**4.2** (monospace family of the xterm body — never touched here), **10.3**
("< 80 network requests": artifact of the Vite dev server, which serves
non-bundled ES modules — the assertion is about a production bundle),
**19.1 / 19.2** (completion popup, untouched area). The global run of the 79
tests in one pass was **discarded**: it was killed by the system for low memory,
and an HMR reload triggered by an ongoing edit had disturbed it — the retained
verification is therefore section by section, on frozen code.

**2026-09-17 — Terminal: no more vertical scroller in the strip, no more
black band under the terminal.** Two distinct display defects, both
measured in the DOM before the fix (no deduction):

1. **Vertical scroller in `terminal-tabs`.** `overflow-x: auto` alone computes
   `overflow-y` as `auto` as well; the `min-h-11` (44px) minus the bottom 1px
   border leaves the content box 1px too short, which is enough to make a
   vertical scrollbar appear. Reading: `clientHeight 43` /
   `scrollHeight 44`, `overflowY: "auto"`. What revealed it is the Tailwind
   migration itself: before it, `.hv-terminal-tabs` carried
   `scrollbar-width: none` (which hid both bars) — a declaration with no
   Tailwind equivalent, therefore lost. Fixed with `overflow-y-hidden`, which
   removes the axis at the source rather than hiding bars: the horizontal
   bar remains available when the tabs actually overflow.
2. **Black band at the bottom of the window.** `terminal-body` had stayed in
   `#1e1e1e` while the xterm theme had gone to opaque white
   (`theme.background: '#FFFFFF'`): `FitAddon` sizes the terminal in
   `floor(height / cellHeight)` lines, so there is always a remainder below
   one cell, which painted the dark background of the container
   under the last line. Reading: `terminal-body` 596px against 594px for the
   xterm, background `rgb(30,30,30)` versus `rgb(255,255,255)`. The drift dates from the
   ongoing Tailwind-on-tokens migration: before it, `.hv-terminal-content`
   carried `rgba(255, 255, 255, 0.55)` — light, therefore consistent with the theme of
   the time — and `#1e1e1e` did not exist anywhere in the file. Fixed by
   aligning the container with the theme; along the way, `bg-[#1e1e1e]` was a
   hardcoded color, forbidden by the "no hardcoded value" rule — it moves
   to the token `--hv-shell-glass`.

Two tests added. **2.9** locks the vertical axis (computed `overflow-y`
`hidden`, vertical overflow ≤ 1px) — the computed value is the only signal
that holds on all platforms, an overlay bar taking no
width. **2.10** compares live the color of the container to that of the
`xterm-viewport` (what the terminal actually paints) instead of a
constant: changing the theme drives the container, the drift is impossible.
These two assertions discriminate well — the values they reject
(`overflowY: "auto"`, `rgb(30,30,30)`) are exactly those read before the
fix. Verified by a real run (Firefox, fresh disposable instance on a scratch
port, dedicated `BASE_URL`, no real data): **81 tests**, sections 2, 4,
8, 12, 13 green, plus the complete suite; only **4.2** fails, pre-existing
(monospace family, already proven so by `git stash` the day before).

**2026-10-07 — the Quotas screen executed and audited live (the last screen
without a net).** The spec written on 2026-09-22 is **green end to end** for
the first time: **38/38** (`npx playwright test tests/e2e/quotas/quotas.spec.ts
--project=firefox`, reference count `--list` = **38**; `grep -cE
'^\s*test\('` = **36**, the difference coming from the responsive loop 360/768/1920, one
line for three tests), against a scratch server (`PORT=8099`,
isolated `DATA_DIRECTORY`, Firefox, no real data, `e2eadmin` seeded via
`POST /api/auth/setup` as in CI — the screen opens as admin). **No dead
selector**: the spec postdates the Appliance migration of
2026-09-12 and already targets the `data-testid` of `Quotas.svelte`, so **no**
assertion had to be fixed. Live audit (same scratch server, real Firefox,
screenshots as evidence): the window opens from the launcher, the header carries
only [ICON] [TITLE], the 3 filter chips (All/Users/Groups) form a
`role="group"` with an `aria-label`, mutually exclusive (`aria-pressed`), the
seeded row renders "USER", the name + "Updated on …", the Upload
`used/limit` bar (measured: 50 MB of 100 MB → `aria-valuenow="50"`, `aria-valuemax`
100, track filled at 50%) and "Unlimited" on the disabled Receive
direction; "Manage in Users" emits `hv-open-app`
(`detail.id = 'users'`), opens the Users window **without leaving**
`/desktop` or closing Quotas; console without errors. Detail in the 2026-10-07
addendum of `CLAUDE.md` (invariant "Explorer / transfers"). Consequence:
none of the 29 screens is now without a live audit **or** without an E2E spec.

**2026-10-12 — deleting a user left its share permissions behind (fixed).**
Found while auditing what was *actually* still open (the docs claimed several
fixed bugs as open, and vice versa). `handle_user_delete`
(`src/handlers/users.rs`) only walked the user's effective permissions to
`tracing::info!("Revoking permission for user …")` — **nothing was revoked**,
neither the POSIX ACL nor the `share_permissions` row. The table keys a grantee
by **name only** (no foreign key to `users`), so the rows outlived the account:
recreating a user with the same username silently inherited the deleted user's
access, and the audit log claimed a revoke that never happened. Fixed by a new
`PermissionService::revoke_all_user_permissions(username, pool)`: it removes
the real ACL for every share the user held a **direct** grant on
(best-effort, like every other revoke — an unresolvable share must not block
the cleanup) then deletes the rows with `delete_for_grantee("user", …)`.
Group grants are deliberately kept — the account was already removed from its
groups earlier in the handler, and the group rows belong to the *remaining*
members. Tests: `tests/users_integration.rs` goes from **60 to 61**
(`test_user_delete_revokes_its_direct_share_permissions_only`: two direct
grants — one on a parseable share type, one on a bogus one — plus another
user's grant and a same-named *group* grant; only the deleted user's direct
rows go). The test was **checked against the old code before shipping**
(`git stash` of the two source files): it fails with `left: 2, right: 0`, so it
really guards the regression. The ACL half needs root + a real share and stays
covered by `tests/e2e/shares/shares.spec.ts` per CLAUDE.md's "What NOT to test
in Rust". The stale "reported not fixed" notes in `docs/guides/shares.md`
(that revoke, plus the two ACL bugs already fixed in code — the `-d -m` order
of `set_default_acl` and the forced execute bit on a directory) were corrected
in the same change.

**2026-10-12 — the progress REST endpoint answered a placeholder, and no `Custom` job could be serialized at all (both fixed).** Two findings from the same subsystem, the second surfaced by the test written for the first.

**(a) `ProgressManager::get_progress` was a stub** (`src/sse/progress.rs`). It
returned a freshly built `Running` / `0 %` / `"Waiting..."` event for whatever
id happened to be in the map, throwing away the `last_event` the manager
already keeps in sync for the SSE stream and the activity feed. So
`GET /api/progress/{job_id}/status` reported "running, 0 %" for a job at 87 %,
for a **completed** job, and for a **failed** one. Nothing consumed it (the
frontend polls `/api/activity` for the taskbar and subscribes to the SSE stream
to follow a job), which is why it went unnoticed. Fixed: the handler returns the
job's real last event; `None` for an unknown or already-removed id.

**(b) `JobType::Custom` could not be serialized — every feature job was
unaffected in the activity feed but broken on the SSE wire.** `JobType` was
declared `#[serde(tag = "type")]` with a `Custom(String)` newtype variant, and serde
cannot internally tag a newtype variant wrapping a bare string: `to_string` fails
with *"cannot serialize tagged newtype variant JobType::Custom containing a
string"*. `Custom` is what **every** feature job uses — Explorer copy/move,
Docker install/update, antivirus scan, RAG reindex, VM create/ISO, ddrescue and
PhotoRec, cloud sync, Ollama model pull — while the built-in kinds
(`Backup`/`Restore`/`Verify`/`Upload`/`Download`/`Sync`/`CacheMover`) are the
ones almost nothing uses in practice. Two consequences, one per consumer:
`handle_progress_sse` does `serde_json::to_string(&event).unwrap_or_default()`,
so a `Custom` job emitted an **empty** `data:` payload — `JSON.parse` throws in
the browser, no `progress` and no `done` callback ever fires, the client falls
through to its `EventSource` error handler, and `Explorer.svelte`'s `watchJob`
therefore resolves **false** for a copy that in fact succeeded (progress bar
frozen, directory listing never refreshed — `Storage.svelte`'s cache mover, a
built-in kind, kept working, which is why the two SSE consumers behaved
differently); and `GET /api/progress/jobs` panicked inside the `json!` macro
for as long as any `Custom` job was tracked (the desktop taskbar was spared: the
activity handler maps `JobType` to a slug through `job_type_kind` and never
serializes the enum — that mapping is why the taskbar audited green all along).
Fixed by switching to **adjacent** tagging, `#[serde(tag = "type", content =
"name", rename_all = "snake_case")]`: the built-in kinds keep their exact
previous shape (`{"type":"backup"}`), and a feature job becomes
`{"type":"custom","name":"explorer_copy"}`. `frontend/src/lib/services/api.ts`
said `job_type: string` (wrong even before, nothing reads the field) and now
describes the real object shape.

Tests: `src/sse/progress.rs` **11 → 20** (5 for `get_progress`: seeded event
before the first emit, mid-flight `%`/message/byte counters, completed, failed,
unknown/removed → `None`; 4 for the serde shape: round-trip of every variant
through JSON, byte-identical wire shape for the built-in kinds, the `custom`
name field, and a whole `ProgressEvent` serializing non-empty) and
`tests/progress_integration.rs` **6 → 7**
(`job_status_reports_the_real_last_event` — over HTTP, through the real router:
mid-flight job reads its own `%`/message/`state`, completed reads
`completed`/100 %, unknown id is a 400; the failing assertion on the old code
was on the serialization half, which is how (b) was found). Both were **checked
against the old code** before shipping: reverting the two source changes makes
the 5 `get_progress` assertions fail (`left: Running` / `right: Completed`,
`left: 0.0` / `right: 87.5`, `left: "Waiting..."`) and the 3 serde ones fail on
`to_string`. `cargo fmt` clean, `npm run check` 0 error. Docs updated in the same
change: `docs/guides/explorer.md` (the "known pre-existing issue" section is
closed), `docs/API.md` (progress section), `CLAUDE.md` (Explorer/transfers
invariant) and `ROADMAP.md`.

**2026-10-12 — static-asset serving: validators, cache lifetimes, pre-compression and a subset icon sprite.** Four optimizations to the embedded `static/` tree (`src/handlers/spa.rs`), none of which changed a route or a payload — only the headers and the bytes behind them.

**(a) No `ETag`, so nothing was ever revalidated.** The `include_dir!`-embedded
files were served as raw bytes with no validator at all, which means a browser
could never issue `If-None-Match`; `/wallpapers/*.webp` (several MB) and the
3.6 MB icon sprite were re-downloaded on every visit. `spa` now builds a
`path → Asset` table once (lazily) and sends an `ETag` — a truncated SHA-256 of
the identity bytes **with a `-br` / `-gz` suffix per content coding**, because a
cache holding the identity body must not validate a `br` request. A matching
`If-None-Match` gets `304` with no body.

**(b) `Cache-Control` was only set on `/assets` and `/fonts`.** New
per-route policy (the blanket `SetResponseHeaderLayer` is gone, so a layer can
no longer accidentally pin the API or `index.html`): `immutable, max-age=1y`
for `/assets`, `/fonts`, `/wallpapers`, `/static`; `max-age=86400` for the two
sprites (their URL is fixed but their content is regenerated every build, so
`immutable` would pin a stale sprite — i.e. blank new icons — for a year); none
for `/manifest.json`, `/favicon.svg`, `/sw.js`, `index.html`.

**(c) Compression ran per request.** `frontend/scripts/precompress-dist.mjs`
(new, npm `postbuild`) writes a `.br` and a `.gz` sibling for every compressible
file in `static/dist`; the same `include_dir!` embeds them and `spa` serves them
on `Accept-Encoding`, setting `Content-Encoding` + `Vary: accept-encoding`. That
header is also what makes `CompressionLayer` skip the response (it never
re-compresses an encoded body), so the on-the-fly compressor is now a fallback.
93 files: 3 375 KB → 748 KB brotli (−78 %) / 846 KB gzip (−75 %). Dropping the
last `SetResponseHeaderLayer` also let the unused `tower-http` features `fs` and
`set-header` go (Cargo.lock loses `http-range-header`, `mime`, `mime-date`,
`futures-util`… from that crate).

**(d) The icon sprite shipped 7 750 glyphs to render ~110.**
`generate-sprite.mjs` now subsets to the files whose string literals can become
an icon name (components rendering `<Icon>`, components passing an `icon`-ish
prop, `$lib/stores` + `$lib/services`, the first-party catalog manifests):
3.6 MB → **55 KB** (11.7 KB brotli) and `mapping-unified.ts` 485 KB → 6 KB.
Scanning the whole tree instead removes almost nothing (7 659 of 7 750 kept)
because icon names are ordinary short words; a whole-tree first attempt was
measured and rejected for exactly that reason. A pre/post comparison script
verified **0** reachable glyph references lost and **0** mapping/sprite
inconsistencies. Regeneration is wired into `npm run build` (`prebuild`) and the
script aborts the build if the subset comes out empty; `HV_SPRITE_FULL=1`
restores the complete sprite.

Tests: `src/handlers/spa.rs` unit **21, new** (the module had none): ETag
shape, stability and per-coding distinctness; `Accept-Encoding` parsing incl.
`q=0`, `*`, case and other parameters; `If-None-Match` incl. `W/`, `*`, lists;
and the whole `asset_response` matrix — br beats gzip, gzip when br is absent, identity
fallback, `Vary`, 304 without a body, a stale validator still getting the body,
and identity-vs-br validator independence. New
`tests/spa_assets_integration.rs` **9** over a real router: the committed
`static/sounds/README.md` gives the deterministic half (validator + `immutable`
+ 304 + no body + a stale validator + 404 rather than the SPA fallback), and the
sprite/wallpaper half states its `static/dist` prerequisite and no-ops without
it (the integration CI job runs `cargo test` with no frontend build). Verified:
`cargo test --lib` **643** ok (622 before this change, `spa.rs` contributing the
21 new ones), `cargo test --test spa_assets_integration` 9 ok,
`svelte-check` 0 errors, `cargo fmt` clean, `cargo clippy` clean on the touched
files (the 3 remaining `manual_contains` warnings are pre-existing in
`src/dependencies.rs`). `npm run build` → 3375 KB raw / 748 KB br.

**2026-10-12 — the Cloud Sync editor read and wrote a different file than the one the remotes come from (fixed).** `handle_get_config`, `handle_save_config`, `handle_add_remote` and `handle_delete_remote` hardcoded `/etc/rclone.conf`, while `parse_rclone_config()` — which backs `GET /rclone/remotes` and every transfer — honoured the `RCLONE_CONFIG` override. Two symptoms for an admin whose config lives elsewhere (the documented way to run rclone as a non-root user): the remote list and the Settings editor disagreed about which file they were describing, and saving failed with `Permission denied` (os error 13) against a root-owned `/etc/rclone.conf`. Both sides now resolve through one function, `rclone_config_path()`: an explicit non-empty `RCLONE_CONFIG` wins outright (existing or not, since that is how a fresh config gets created outside `/etc`), then the first of rclone's fallback locations that exists, then `/etc/rclone.conf`. `parse_rclone_config` delegates to it instead of looping over candidates on its own. Tests: `handlers/rclone.rs` **11 → 14** (override wins even when the file does not exist yet; an exported-but-empty override is ignored rather than resolving to `""`; the fallback is always absolute; and the parser and the handlers resolve the same file) and `tests/rclone_integration.rs` **14 → 17** — the suite could never test the config CRUD because a writable `/etc/rclone.conf` was out of reach, so `add_remote → list → delete_remote` plus `GET`/`POST /config` and the `.backup.<timestamp>` sibling now run for real against a tempdir-selected config. Checked against the old code: the three integration tests fail with `Failed to read config /etc/rclone.conf: No such file or directory` and `Failed to save config: Permission denied`, i.e. exactly the reported inconsistency. Note the list endpoint itself was already correct — the bug was confined to the four read/write handlers. `cargo fmt` clean, `cargo clippy --lib -- -D warnings` clean. (The 3 `manual_contains` warnings in `src/dependencies.rs` — from the parallel `install.sh` refactor — were fixed in the same pass: `iter().any(|have| *have == want)` → `contains(&want)`. CI runs `cargo clippy --all-targets -- -D warnings`, so they were a hard failure, not a style note.)

**2026-10-12 — the RAG knowledge base was missing 25 of the 45 indexable guides (fixed).** `CLAUDE.md` has always required "a new feature gets a `FeatureDoc` + a `rag.md` Document Sources row", but the rule was **prose only** and was never applied retroactively: `src/rag_indexers.rs` carried **20** entries and `docs/guides/` had grown to 46 files. The consequence was user-visible — the assistant answered "I don't know about that" for shipped features such as Syncthing, the downloader, notifications, the desktop shell and app launcher, LUKS encryption, the RAID wizard, the cache tier, widgets, wallpapers, themes, mDNS, server control, avatars, the account page and the setup wizard. All 45 are now indexed (**20 → 45** entries). The change also **removes the class of bug** rather than just the instances: `test_every_guide_has_a_feature_doc` enumerates `docs/guides/*.md`, subtracts the explicit `DEVELOPER_ONLY_GUIDES` list (just `design-system.md` — a guide about tokens is not a feature a user can open), and requires the result to equal the guides claimed by the `FEATURE_GUIDES` table id-for-id, itself asserted against the real `feature_docs()`; `test_feature_docs_are_unique_and_substantive` rejects a duplicate id, a stub body (< 200 chars) and implementation details leaking into the text (`src/`, `.svelte`, backticks), since these entries are read by a user-facing assistant. Verified by mutation: deleting the `avatars` row fails the coverage test with the two guide lists side by side. Two tests replaced the previous `test_feature_docs_are_nonempty`, which asserted `features.len() >= 18` on a **hardcoded list unrelated to the shipped vector** — it could not have caught the drift, and hadn't. The Users entry was also refreshed: it still described account deletion without the share-permission revoke. `cargo test --lib rag_indexers` **3/3** (`index_all` still upserts on re-index without duplicating).
**2026-10-13 — the first-run state moved from marker files into the database, and the hardware-change dead-end became a recovery flow.** `/etc/homevault`-style marker files (`.setup_complete`, `.system_hash`) answered "is the appliance configured?" while the admin account lived in SQLite, so the two could disagree: losing the data directory while the DB survived re-opened the Setup Wizard with an admin still present, and a changed hardware fingerprint re-showed the Wizard only for `/auth/setup` to reject it with "Setup already complete". Now `SetupStateRepository::has_admin()` (a `users` row with `role_id = 2`) is the single source of truth, and the singleton `setup_state` row (migration **037**, `sqlx/migrations/037_setup_state.sql`) carries the fingerprint recorded at setup time — the account and the setup state travel together in `{DATA_DIRECTORY}/data/homevault.db`. A changed fingerprint with an admin present sets `needs_recovery`, which the new frontend route guard (`frontend/src/lib/stores/appGate.ts`, the standard route-guard pattern) routes to a `Login` `recoveryMode` screen; a successful admin sign-in (step-up auth) calls the new admin-only `POST /auth/setup/rebaseline`. The three unauthenticated bootstrap routes (`/auth/setup`, `/auth/setup/verify-totp`, `/auth/check-username`) are now restricted to loopback/private/link-local peers by `src/middleware/setup_guard.rs` (peer from `ConnectInfo`, never a forgeable `X-Forwarded-For`; `HOMEVAULT_SETUP_ALLOW_REMOTE=1` opts out). Kept: Argon2, the 8+/mixed-case policy, `check-username`, the one-time recovery code, TOTP at the password step, and the `O_EXCL` setup lock. The reference-design reboot-during-setup RAID flow is deliberately **not** implemented (the wizard has no storage step; pools are created after setup) — documented in `docs/guides/setup-wizard.md`. Tests: `setup.rs` **43 → 45** unit, `setup_state_repo.rs` **6**, `setup_guard.rs` **6**, `setup_integration.rs` **34 → 47**, `login_integration.rs` **53 → 54**; `cargo test --lib` **656/656**, `npm run check` 0 errors, `npm run build` OK. Frontend E2E specs updated but **not re-run** (no fresh first-run server in this session).

## Summary

**2026-10-21 — the E2E suites never re-checked the first-sign-in password gate
they walked into.** The Users/Groups pass added `middleware::password_change_guard`:
an account created through `POST /api/users` holds an admin-chosen password, so
**every** API call carrying that session answers `403 password_change_required`
until the owner sets their own. Fourteen E2E specs still built a throwaway
non-admin (and, in `account.spec.ts`, a throwaway admin) with `POST /api/users`
and then used the returned session directly. Two different failure modes, both
invisible in a pass/fail count:

- **Red** — the tests asserting a *successful* call: `chat` 4.3, `cloudsync` 4.3,
  `downloader` 10.3, `storage` 4.4, `syncthing` 3.3 and `mcp` 15.5 failed on the
  403 (six specs).
- **Green but vacuous** — the tests asserting a *refusal*: `vpn`, `network`,
  `shares`, `rescue`, `explorer` 10.x, `raid` 10.2 and `security` 6.3 got their
  403 from the forced-change gate instead of the admin gate they were written
  to prove, and `login` 24.2/24.3 quietly *skipped* because the recovery-code
  generation was refused, dropping the only end-to-end cover of the
  recovery-code reset.

Every creator helper now clears the flag before the session is used — log in,
`PUT /api/users/{u}/password` with the current password, log in again (the change
revokes the flagged session) — and hands back the owner-chosen password so
`enrol2fa` and the account forms keep using a live credential:
`apiLoginAsViewer()` in `chat`, `cloudsync`, `downloader`, `storage`, `syncthing`,
`mcp`, `vpn`, `network`, `shares`, `rescue`, `security`; the creator itself in
`account`, `explorer`, `raid`, `users`; and `login` 23.1 now asserts the real
ordering (flagged member sees the change screen, completes it, *then* is refused
as not-admin) while 24.x clears the flag before generating the recovery code.
Two spec-level bugs surfaced by the same re-run are fixed: `account.spec.ts`'s
`gotoDesktop` dismisses the once-per-version "What's New" modal (every account it
creates is brand new, so the overlay intercepted the user-menu click), and
`login` 25.3 no longer assumes the *fifth* submit is the one that locks — the
per-IP throttle (30/min, 120/hour, shared by the whole suite) can absorb an
attempt before it reaches the per-account counter, so it retries until the lock
copy appears (the five-attempt threshold itself stays pinned by the Rust suite).
Verified: `login` **101 passed**, `account` **74 passed**, `backup` **29 passed**
(real restic lifecycle included), `explorer` §10 6/6, `raid` §10 3/3, and the
security sections of `chat`/`cloudsync`/`downloader`/`storage`/`syncthing`/`mcp`
**25 passed / 1 skipped**, `vpn`/`network`/`shares`/`rescue`/`security`
**54 passed / 10 skipped** (Firefox, scratch server `:8102`). `users.spec.ts`
cannot run on this host (`POST /api/groups` needs passwordless `sudo` for
`groupadd`); its `createUser` helper got the same fix.

**2026-10-21 — Firewall/ufw closed to production-ready: nothing unvalidated
reaches `sudo ufw`, and every destructive action is confirmed and verified.**

Seven holes, the first three of them exploitable from a plain admin session:

1. **Client input was interpolated straight into the `ufw` argv.** `action` is
   the *verb* of `sudo ufw <action> …`, so `{"action":"delete"}` on the "add
   rule" endpoint ran `sudo ufw delete …` (`--force`, `reset`, `enable` were
   equally reachable). `protocol`, `port` and `source` went through as-is: a
   port of `22/tcp` was concatenated a second time (`22/tcp/tcp`), and
   `source: "any"` turned a "restricted" rule into a global one. New
   `validate_action` / `validate_protocol` / `validate_port` /
   `validate_source` / `validate_comment` + the default-policy pair, all pure
   and unit-tested, run **before** the `ufw_available()` probe — a typo is
   reported as a typo, and no process is spawned.
2. **A source-restricted rule could not be deleted, and the global rule with
   the same port was deleted instead.** `delete_rule_by_spec` rebuilt
   `delete <action> <port>/<proto>`, which does not match `allow from <cidr>
   to any port <n> proto <tcp>`. `FirewallRule` now carries `spec` — the exact
   argument string `ufw show added` printed, comment stripped — and the
   handler passes *that* to `ufw delete`. The client only selects a live rule;
   it never supplies the command.
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
handlers had always recorded and nothing ever read (`list_change_log` was dead
code) with a new `default_policy_changed` action. The card gains the
default-policy row and dialog, a "Recent changes" list, one focus-trapped
confirmation dialog shared by enable/disable/remove/policy (it was a
`role="presentation"` div inside a `role="button"` overlay with no focus
handling), labelled inputs, an `aria-label` per row's remove button, and a
`role="status"` line reporting every outcome — failures used to land in an
empty `else {}`.

Tests: `src/firewall.rs` **9 → 31** unit, `src/handlers/firewall.rs` **0 → 2**,
`security_integration.rs` **18 → 25** (both confirmation gates, the pre-ufw
rejections, the 404/403 delete paths, the history envelope),
`security.spec.ts` **30 → 41** `test(` blocks (39 → 43 runtime tests) — the new
§5b intercepts the firewall endpoints with `page.route`, so the card's default
policy, focus-trapped remove dialog, `spec` payload, disable gate and error
feedback are covered on a host where `sudo ufw` wants a password. Verified:
`cargo test --lib firewall` **31/31**, `cargo test --test security_integration`
**25/25**, `security.spec.ts` **35 passed / 8 skipped / 0 failed** (Firefox,
scratch server `:8102`, `DATA_DIRECTORY` under `/tmp/opencode`),
`npm run check` 0/0, `npm run build` OK. No live `ufw` mutation: `sudo -n true`
fails on this machine, so `ufw_available()` is `false` and the live
enable/disable cycle stays documented from the 2026-07-16 signed-off audit.

**2026-10-21 — Backup/restic closed to production-ready: the run is a real
background job, its progress is real, and no unvalidated value reaches
restic's argv.**

Five holes, each one visible to a user who believed their data was safe:

1. **The backup ran inside the HTTP request.** `handle_restic_backup_create`
   and `handle_restic_restore` blocked until restic exited — hours on a
   multi-TB dataset. The browser saw a stalled request, `trackJob` was only
   called once the run had *already* finished (so the floating island showed a
   completed job from the start), and a Tokio worker was parked for the whole
   run. Both endpoints now answer immediately with
   `{status:"running", job_id, backup_job_id}` and the run continues in a
   spawned task, the blocking restic call on `spawn_blocking`
   (`ResticService::run_restic_op`). Every other restic call (snapshots,
   stats, unlock, prune, forget, restore-plan) goes through the same helper
   with a timeout — `RESTIC_QUERY_TIMEOUT` 120 s for metadata,
   `RESTIC_INIT_TIMEOUT` 300 s for `init`, none for the data-moving
   operations, which rely on cancellation instead.
2. **The progress bar was fabricated.** `create_backup` emitted 5 % then
   jumped to 90 % regardless of the run. `restic backup --json` is
   newline-delimited JSON (`{"message_type":"status",…}` objects then one
   `summary`), and it is now read line by line
   (`create_backup_streaming`): `percent_done` / `bytes_done` become real SSE
   progress (`ResticProgress` → `progress_extended`, mapped to 1–99 % so 100 %
   waits for the summary line).
3. **A healthy repository was reported broken.** `check_repo` collected every
   output line containing `error`, and `restic check` prints **"no errors were
   found"** on success — so `healthy` was `false` for *every* healthy
   repository, and the UI told the user their backup was corrupt. The verdict
   now comes from `parse_check_output`: the success sentinel is excluded, only
   `error:` / `fatal:` / `unable to` / `failed to` / `is damaged` lines count,
   and a non-zero exit with no recognizable marker still reports one error.
4. **Argument injection could wipe a repository.** `snapshot_id` was passed
   positionally to `restic forget`, so
   `{"snapshot_id":"--keep-last=0"}` was parsed by restic as an *option* and
   forgot every snapshot in the repository — a delete request became a wipe.
   `validate_snapshot_id` (`[A-Za-z0-9._-]`, no leading `-`) plus
   `validate_backup_path` / `validate_restore_target` (absolute, no control
   character, no `..`) / `validate_snapshot_name` (it becomes
   `/tmp/restic-snap-<name>`, so `../../etc` used to mount over an arbitrary
   directory) now refuse the value before restic is spawned. Same for
   `validate_repository`, `validate_password`, `validate_display_name`,
   `validate_keep_within`, `validate_tags` and `validate_exclude_patterns`;
   repository create/update additionally refuse an empty name, an empty
   location, an empty password, a relative source path, an unknown
   `retention_policy_id`, and `snapshot_create_before_backup` without a pool
   and an absolute dataset.
5. **A failed snapshot mount was ignored.** With
   `snapshot_create_before_backup`, a mount that failed was logged and the run
   continued against an empty mount point: the backup captured nothing,
   reported success, and deleted the snapshot — the worst possible outcome for
   a backup tool. `SnapshotMount` (RAII) aborts the run on a failed mount and
   always unmounts/removes the directory on the way out.

The run itself is now supervised:

- **One backup/restore per repository** — `RepoRunLock` (shared by the manual
  handlers and the scheduler, released by `Drop`) answers "A backup or restore
  is already running on this repository" instead of letting restic say
  "repository is already locked" *after* a full source scan. Two different
  repositories may still run in parallel.
- **Cancellation really stops the process** — the child is registered per job
  and the cancel hook sends **SIGINT** (restic then releases its repository
  lock), with SIGKILL after 10 s as a watchdog; the job ends `cancelled`
  (`ResticError::Cancelled`), not `failed`.
- **The job row exists before the run** (`status = running`), so `started_at`
  is the real start time and the run is visible while it happens; `snapshot_id`
  is filled in when restic reports it (`update_backup_job` now takes it).
  `fail_stale_running_jobs()` runs once at startup: a crash or a
  `systemctl stop` mid-backup used to leave a phantom `running` job in the
  history for ever.
- **`max_size_gb` is finally applied.** It was stored, returned by the API and
  **never read** by the scheduler. restic has no size-based `forget`, so
  `plan_keep_last_for_size(total_size, count, cap)` derives a `--keep-last` from
  the average snapshot size and the scheduler runs `forget --prune` with it.
  It never returns 0 and never deletes the newest snapshot: a single snapshot
  over the cap is left alone with a log line rather than wiped.
- **Retention policies are validated**: every limit must be > 0
  (`max_count = 0` meant "forget everything"), a *create* needs at least one
  limit (a policy with none silently never pruned), and only one policy may be
  `apply_to_all` (`get_default` used to pick one arbitrarily by name).
- **`check --read-data` is admin.** It re-reads every pack file — hours of
  disk I/O — so it no longer sits behind a plain session; the metadata check
  stays `require_auth`. `forget` with no rule at all is refused here instead of
  surfacing restic's "Fatal: no policy was specified".
- **Frontend**: `trackJob(jobId, label, onSettle?)` reloads the
  snapshots/jobs/stats/reminders when the job really settles (reloading at
  response time would show the *previous* state now that the endpoint answers
  before the run); the retention-policy form gained the `max_size_gb` field
  (i18n `max_size_gb_label` / `policy_summary_size` in en/fr/de/es) and
  "Apply retention" is disabled when no bucket is set.

Tests: `restic.rs` **8 → 29**, `restic_scheduler.rs` **12 → 16**
(`plan_keep_last_for_size`), `handlers/restic.rs` **3 → 6**
(`validate_retention_limits`), `restic_integration.rs` **17 → 24** (repo create
validation *before* the availability probe, relative source path, busy
repository, `forget` without a rule / bad `keep_within`, option injection on
`snapshot_id` and on the restore target, `read_data` admin, zero and
second-default policy, stale-job reaper + `snapshot_id` recorded on
completion), `backup.spec.ts` **25 → 27** (3.6 — no-limit and zero-count policy
refused; 13.3 — a freshly written repository checks out `healthy`; 13.1 adapted
to the async contract via `waitForBackupJob`). `cargo test --lib restic`
**51/51**, `cargo test --test restic_integration` **24/24**, `npm run check`
0 errors / 0 warnings, `npm run build` OK, `cargo clippy --all-targets` clean
on every touched file. The §13 real-restic E2E **skips on this host**: `restic`
is not installed here — they remain the live-host check and were not run in
this session.

**2026-10-21 — Users/Groups closed to production-ready: deleting an account
now really removes it, and demoting an admin really demotes it.**

A HomeVault account can be backed by a real Linux account (a PAM sign-in
mirrors one, an admin promotion provisions one). Nothing in the app ever
touched that host half, which produced four security holes:

1. **Delete left a working login.** `DELETE /users/{u}` dropped the row only.
   A deleted admin kept its Linux login *and* — because admin status is derived
   from `homevault_admin` membership — a working HomeVault **admin** login that
   the next PAM sign-in silently recreated. The handler now removes the Linux
   account **before** the row, so a refusal is retryable and leaves both sides
   consistent.
2. **Demotion left real sudo.** Only `homevault_admin` was cleared; `sudo` and
   `wheel` stayed, and the session kept the `is_admin` snapshot it was created
   with (up to 30 days with "remember me"). `HostAccountService::revoke_admin`
   now clears all three **before** the role write, `AuthService::update_role`
   signs the demoted account out, and `get_session` re-derives `is_admin` from
   the current role on every call.
3. **Lock left the host password usable** (SSH and the web terminal ignored the
   DB flag) — now mirrored to `usermod -L/-U`.
4. **Password reset was a no-op for a PAM-mirrored account** (sentinel hash) —
   the new secret is now pushed through `chpasswd` on stdin.

New module `src/host_accounts.rs` (`HostAccountService`) owns all of it:
username gate before any command, system-account refusal (`uid < 1000` or
`>= 65534`), 20 s `HOST_OP_TIMEOUT` on `tokio::process` with `kill_on_drop`.
`userdel` is deliberately not `-r`: home removal follows the explicit
`delete_home_folder` choice and the path HomeVault recorded.

Groups hardening in the same pass:

- `create_group` / `add_user_to_group` provision a Linux account per member, so
  a member must now be an existing HomeVault user (`400 "User '<x>' not
  found"`, checked before any host command); blank names and >500 members are
  refused (`GroupService::normalize_member_list`).
- `DELETE /groups/{name}` now purges **all four** display-name references
  (`hv_groups`, `transfer_quotas`, `resource_group_shares` and — new —
  `share_permissions`), matching what a rename already migrates. A group
  re-created under the same name inherited the old group's quota and share
  ACLs before this.
- Every handler call into `GroupService` goes through
  `crate::groups::run_group_op` (`spawn_blocking`, 15 s ceiling): a hung
  `getent` (NSS/LDAP) or `groupadd` on a locked `/etc/group` used to stall a
  Tokio worker, and the Users list ran one blocking `id -nG` per row on the
  async runtime.

Tests: `host_accounts` unit **14/14**; `users_integration` **80/80** (71 → 80);
`groups_integration` **38/38** (+4); `cargo check --tests` OK. The six real
host effects (grant/revoke admin groups, `chpasswd`, `usermod -L/-U`,
`userdel`, system-account refusal) are `#[ignore]`d root-only tests in
`tests/login_pam_integration.rs` — see the checklist below.

**Live-host checklist** (disposable machine, run as root):
`sudo -E cargo test --test login_pam_integration -- --ignored --nocapture`.
Expected: `grant_admin_provisions_the_account_and_every_admin_group`,
`revoke_admin_clears_every_admin_group`, `set_password_rotates_the_host_credential`,
`set_locked_locks_and_unlocks_the_host_credential`,
`delete_account_removes_the_linux_account_but_keeps_its_home`,
`delete_account_refuses_a_system_account` — 6/6, each cleaning up the throwaway
Linux account it creates. Not exercised in this session: no disposable host
account was available, and `userdel`/`usermod` on the workstation's real
accounts is out of bounds.

**2026-10-21 — Rescue closed to production-ready: the "Cancel" that cancelled
nothing now stops the tool, the status endpoint cannot read arbitrary files,
and a clone can no longer be pointed at a live or smaller disk.**

The Rescue app is the one screen allowed to write to raw block devices, and it
had four real gap classes. Before this pass:

1. **A cancel that only cancelled the *display*.** `DELETE
   /api/progress/{job_id}` dropped the job from `PROGRESS_MANAGER` and answered
   "Job cancelled" — while `sudo ddrescue` kept writing to the destination disk
   and `photorec` kept carving. Nothing in the process ever observed the
   request. The UI had no stop control at all, so a clone of a failing 4 TB
   disk could only be aborted by killing the server.
2. **Arbitrary-file read as the service user.**
   `GET /api/rescue/ddrescue/status/{logfile}` passed the raw URL segment to
   `tokio::fs::read_to_string` with no validation of any kind: any admin
   session could probe `/etc/shadow` (a 200/400 answer is a file-existence
   oracle, and the parsed `bytes_total` leaks the size).
3. **Destructive clones with no safety net.** Nothing checked that source and
   destination were different devices, that the destination was not currently
   mounted / an active md-LVM-crypt member (a member disk with no mountpoint of
   its own has no `lsblk` row but does have sysfs `holders`), or that the
   destination was at least as large as the source. `PROTECTED_DEVICES` was a
   hardcoded `sda`/`nvme0n1`/`vda` list, so a host whose root disk is
   `/dev/sdb` happily let a clone overwrite it.
4. **Options that lied.** TestDisk's `deep_search`/`log_path`/`force`,
   PhotoRec's `keep_broken`/`max_size` and every other unvalidated value were
   accepted, stored, shown as applied in the UI and never reached the tool.
   `mdadm`'s `action` — offered as a free-form select — was forwarded verbatim
   into `--action=<value>`. `file_types` were spliced into PhotoRec's
   **comma-separated** `/cmd` grammar, so `"jpg,search"` injected a command.
   And no subprocess except the TestDisk scan had a timeout.

The pass (`src/rescue/validator.rs` v2.0.0, `src/rescue/jobs.rs` new,
`src/rescue/models.rs`, `src/rescue/service.rs`, `src/handlers/rescue.rs`,
`src/sse/progress.rs`, `src/handlers/progress.rs`):

1. **Real cancellation.** `ProgressManager` gained
   `register_cancel_hook`/`clear_cancel_hook`/`cancel_job` (+
   `CancelOutcome::{NotFound,NoHook,HookRan}` and `mark_cancelled`, emitting
   the `ProgressState::Cancelled` that existed but had never been used). The
   generic handler runs the hook and answers `{job_id, cancelled, message}`,
   with a `400` for an unknown id instead of a phantom success. `jobs.rs`
   keeps the registry of rescue jobs that own a live process: both tools are
   spawned with `process_group(0)` and cancelling runs `sudo kill -s TERM --
   -<pgid>`, escalating to `SIGKILL` after 10 s — signalling only the `sudo`
   wrapper would leave the real tool writing to the disk. A cancel that lands
   between `register` and `spawn` is not lost (the flag is set first, the kill
   is issued as soon as the pid exists). A cancelled clone is reported as
   cancelled, not failed, and its mapfile is kept so it can be resumed.
2. **Confined status reads.** `validate_readable_log_path` requires an
   existing regular file under `/tmp`, `/var/tmp` or `paths::rescue_dir()`, no
   symlink anywhere (final *and* parent components are checked — a planted
   `/tmp/x -> /etc/shadow` must not be read through), and ≤ 16 MiB.
   `validate_output_path` got the same treatment plus the component-wise
   containment fix (`/tmpfoo` is not inside `/tmp`).
3. **Clone safety net.** `check_clone_safety` (pure, unit-tested) refuses the
   same device, an in-use destination (`lsblk` mountpoints on the device *and
   its children*, plus sysfs `holders` for md/LVM/dm-crypt use) and a
   destination smaller than the source; an unreadable size degrades to a
   warning. `validate_block_device` now finds the **live** root disk through
   `findmnt /` and matches device names partition-aware (`sda1` belongs to
   `sda`, `sdaa` does not — a plain `starts_with` conflated them).
4. **Honest options.** `DdrescueOptions::validate` (block size a power of two
   in 512..=1 MiB, retry passes ≤ 100 000), `PhotoRecOptions::validate`
   (lower-case identifier file types ≤ 64, partition type whitelisted),
   `MdadmOptions::validated_action` (whitelist, `rewrite` → `repair`), and the
   un-honourable fields are **rejected with a 400 naming them** rather than
   ignored — TestDisk's `deep_search`/`log_path`/`force` and PhotoRec's
   `keep_broken`/`max_size` are gone from the API and the UI. Every subprocess
   now runs through `RescueService::run_output` (kill-on-drop + timeout:
   tool probes 10 s, `lsblk` 15 s, TestDisk analyse 120 s, TestDisk scan 300 s,
   mdadm 60 s). The default mapfile moved out of world-writable `/tmp` into
   the root-owned `paths::rescue_dir()` (ddrescue writes it as root), and
   PhotoRec's recovered tree is `chown -R`'d to the requesting admin — the
   carve runs as root, so the files were otherwise unreadable to them.

Tests: `rescue` unit **8 → 44** (validator hardening, clone-safety matrix,
option matrices, partition-suffix handling), `sse::progress` unit **20 → 26**
(the hook lifecycle: runs once, taken hook, unknown id, removal), `lib`
**965 → 999**; `rescue_integration` **28 → 38** (confined status paths, symlink
and directory refusals, a legitimate mapfile, the mapfile-directory route, all
six rejected options); `progress_integration` 7 → **8** (the cancel endpoint's
three outcomes). E2E `rescue.spec.ts` 53 → **59** (three status/option
rejection tests, a cancel test, an updated TestDisk-tab test that now asserts
the advanced toggle is *absent* there). `cargo fmt` clean; `cargo clippy
--lib --tests` adds no warning in the touched files; neighbouring suites re-run
green (`raid_integration` 5, `storage_integration` 28, `api_contract` 2,
`progress_integration` 8). `npm run check` 0/0, `npm run build` OK.

Also fixed in the frontend: a PhotoRec completion matched the recovered path
out of the message and threw it away (`const path = msg.match(...)` unused), so
the user was told "recovery completed" with no way to find the files.

Still to verify on a real host (cannot be exercised in the sandbox — it has no
spare block device, and a real clone or cancel would touch live disks; listed
under "Remaining to test as a priority" below): a real device-to-device clone
under `--force`, cancelling a running `sudo ddrescue` and confirming the tool
actually disappears (`kill -0 -<pgid>` after the grace), resuming from the kept
mapfile, the PhotoRec `chown -R`, and the in-use guard against a genuinely
mounted destination.

**2026-10-21 — Shares closed to production-ready: every share field is
validated before it reaches a root-owned config file, `/etc/exports` is
rewritten line-exactly, and the whole module left the async runtime.**

The Shares app was the last screen whose inputs still reached system
configuration unvalidated. Concretely, before this pass:

1. **Config injection (root).** `create_smb_share` formatted
   `share.name`/`comment`/`path` straight into `/etc/samba/smb.conf`, so a
   name containing a newline forged an extra section — `[x]\n include =
   /tmp/evil.conf` is root code execution at the next Samba reload — and a
   `]` broke out of the header. `create_nfs_share` wrote
   `path *({options.join(",")})` with both operands user-controlled: a
   whitespace in the path shifted the `/etc/exports` columns, and an option
   `rw) /etc *(no_root_squash` closed the client list and fabricated a second
   export. `apply_ftp_config` interpolated `welcome_message` into
   `ftpd_banner=`, so a multi-line banner appended `anonymous_enable=YES`.
   The SFTP jail wrote `share_path`/`mountpoint` into `/etc/fstab`
   (`ensure_fstab_entry`) with no check at all.
2. **NFS state that did not exist.** `set_share_enabled(Nfs, true/false)` ran
   `exportfs -o rw /<name>` / `exportfs -u /<name>` — a wrong-shape call that
   changed only the in-memory export table, so a "disabled" share came back on
   the next reload or reboot while the UI kept showing it disabled; a bogus
   `exportfs -u /{name}` also preceded every delete. Delete/update matched on
   the path **basename**, so `/data/media` and `/backup/media` silently deleted
   or rewrote each other.
3. **Blocking call sites.** ~20 `SharesService` calls (file IO + `sudo
   systemctl`/`tee`/`exportfs`) ran inline in async handlers, and
   `rebuild_jails` ran `useradd`/`mount --bind`/`chpasswd` inline inside an
   `async fn`, none with a timeout.

The pass (all in `src/shares.rs`, `src/sftp.rs`, `src/handlers/shares.rs`):
pure validators (`validate_share_name` — charset, length, no `..`, Samba
reserved sections; `validate_share_comment`/`validate_smb_config`/
`validate_ftp_config` — single-line values, octal masks, `security_mode`
whitelist, `passive_ports` shape; `validate_share_path` — absolute, no
whitespace/control/`( ) , # ;`/`..`; `validate_nfs_options` — no whitespace or
`( ) , # " '`), called from `create_share`/`update_share`/`delete_share` and
`save_smb_config`/`save_ftp_config`, i.e. **before any privileged write**, for
every caller (handlers, `cache.rs` export sync, ransomware auto-disable). The
SFTP side reuses them for `ensure_fstab_entry` and for grantees' usernames,
and `set_authorized_key` rejects a path-fragment username before `useradd`.
`/etc/exports` now goes through one parser/generator pair
(`parse_nfs_export_line`/`nfs_export_line`): the toggle writes or strips a
`# homevault-nfs-disabled:<name>` tag (durable state), delete/update rewrite
exactly one resolved line, and an ambiguous basename is a `400` naming the
colliding paths. `run_share_op` (`spawn_blocking` + 30 s) wraps every service
call, `rebuild_jails` is split into an async DB phase and a blocking
mount/account phase under a 120 s ceiling, `list_grantees`/`set_authorized_key`
are wrapped the same way, and `save_ftp_config`'s banner is additionally
sanitised at generation time (defence in depth).

Tests: `shares.rs` unit **14 → 34** (validators accept/reject matrices, the
exports line/tag round-trips, the SMB section shape), `sftp.rs` unit
**6 → 9** (fstab column/line injection, path-fragment usernames),
`shares_integration.rs` **20 → 26** (name/comment/path/options injection,
multiline SMB global config, FTP banner + port shape — all `400` before any
`sudo`). `cargo test --lib` **942 → 965**; `cargo fmt` clean;
`cargo clippy --all-targets` adds no warning in the touched files. Re-ran the
neighbouring suites green: `resource_shares` (15), `share_links` (12),
`api_contract` (34), `users` (2), `groups` (3), `mdns` (71).
`npm run check` 0 errors / 0 warnings, `npm run build` OK (frontend untouched
— no UI change was needed).

Still to verify on a real host (cannot be exercised in the sandbox, listed
under "Remaining to test as a priority" below): a real SMB/NFS/vsftpd reload
after a create, a disabled NFS export surviving a reboot and being re-enabled
from the UI, two exports with the same basename producing the new `400`, and
an SFTP grant writing a bind entry that `mount -a` accepts.

**2026-10-21 — VPN closed to production-ready: the OpenVPN start verifies the
tunnel, the uploads are content-checked and the public-IP lookups are cached.**

The VPN module still carried three real gap classes: (a) `openvpn --daemon`
exits 0 immediately, so a start answered "started" while the daemon quietly
retried forever (bad credentials, unreachable endpoint, broken profile);
(b) an uploaded `.ovpn` was executed **as root** with no content check — the
exec-hook directives (`up`, `client-connect`, `auth-user-pass-verify`, …,
`script-security > 1`) are a privilege escalation if an upload session is
compromised; (c) every server-status poll and peer provisioning contacted
three external echo endpoints with no cache. The 2026-10-21 pass, verified
empirically against OpenVPN 2.6.19 (`/tmp/opencode/ovpntest` sandbox):

1. **start → verify → rollback** (`start_openvpn_connection`): the daemon
   launches with a per-connection `--log` and `--cd /etc/openvpn`
   (`--daemon` chdir()s to / — relative paths inside the profile break;
   the client-side `--status` file carries no connection state, only
   statistics, verified empirically), then the log is polled (250 ms) for
   up to **10 s** for `Initialization Sequence Completed`; a fatal
   (`Options error` / `Exiting due to fatal error`) short-circuits the wait;
   on timeout *or* fatal the daemon is killed (rollback) and the API names
   the log's error line (timestamp stripped). Starting an already-running
   profile is idempotent success. Credentials go to
   `/etc/openvpn/openvpn-<name>.auth` (0600, **kept while the connection
   runs** — OpenVPN re-reads them on renegotiation — deleted on stop/delete;
   never `/run`, not writable by the service);
2. **profile deny list** (`validate_openvpn_config_content`, enforced in the
   engine at save and at handler level → 400 naming the directive) plus a
   **409** on uploading over a profile whose daemon is currently running
   (`is_openvpn_running_for_config`, `pgrep -f` on the full config path —
   `pgrep -f openvpn.*<name>` also matches the wrapper shell, another
   empirical finding); OpenVPN and WireGuard handlers wrap the blocking
   engine in `spawn_blocking` (a start may hold the full grace), stop/delete
   remove the `.log`/`.auth` defence files, and start failures are
   `log_event`-warned;
3. `ensure_wireguard_server` now verifies `wg0` is actually up after
   `wg-quick up` (300 ms grace) instead of assuming the silent wg-quick
   success;
4. **`PublicIpCache`** (`src/network.rs`): 15 min positive TTL / 60 s
   negative, injected-instant unit tests, `HOMEVAULT_PUBLIC_IP` override
   still bypasses.

Frontend: the OpenVPN card's start button shows a disabled spinning
`connecting` state while the verified start is in flight
(`vpn.connecting` ×4 locales). Tests: `src/vpn.rs` unit 14 → **29** (16 new:
the whole deny-list matrix, log-parser trio, error-summary timestamp
stripping), `tests/vpn_integration.rs` 22 → **25** (three 400
content-validation tests), `front-end check` green; `cargo fmt` clean,
`cargo clippy` clean on the touched files (`cargo test` full run green:
lib **942**, all integration suites green); live tunnel behaviour
(WireGuard handshake and a real OpenVPN convergence + an auth failure
watched through the verify) stays on the list below.

**2026-10-21 — Network closed to production-ready: the persisted configuration
is now applied, verified and rolled back in the same request.**

The 2026-10-20 persistence pass ended with the config "written, **never
applied**" — `netplan apply` right after the live `ip`/`dhclient` mutations
would tear the interface down under the admin's own session, so runtime and
boot state were allowed to differ by one re-apply cycle and the admin could
never see, in the session, whether the file was loadable and equivalent to the
wire. `PUT /network/interfaces/{name}/config` now runs the full
apply-verify-rollback pattern (`src/network.rs::apply_persisted_config`,
driven by the handler inside one `spawn_blocking`):

1. **Capture** the pre-change live state (`get_interface_config` — the
   rollback target) *before* anything is touched;
2. **live apply** (unchanged; on failure nothing has been touched);
3. **persist** — `persist_interface_config` now returns `PersistedChange`
   carrying the backend *plus the pre-change snapshot of our own netplan
   file* (read via `sudo cat`, it is root-owned 600 after the first write);
4. **reconcile** — `netplan apply` re-asserts the just-written settings, then
   the wire is verified with a 10 s polling grace (250 ms steps): static v4
   address present, default route `via` the configured gateway, static v6
   `addr/prefix` present, interface still up — the verdict is the pure
   `check_live_state_matches`;
5. **on any failure, roll back both halves** — restore the previous netplan
   file (or delete ours when none existed, `sudo rm -f` scoped to the
   `99-homevault-*` glob), `netplan apply` again, then re-apply the previous
   live configuration via `apply_interface_config` — the step that keeps the
   operator session alive — and answer `success:false` naming what failed;
   `Network.svelte` reloads the interfaces so the cards show the rolled-back
   wire. A reconcile failure is logged to the event journal.

Supporting changes: the sudoers grants gained the scoped shapes `netplan apply`,
`chmod 600 /etc/netplan/99-homevault-*.yaml`, `rm -f …` and `cat …` (the
legacy non-root service mode; the root systemd unit routes through sudo
transparently); `netplan.io` joined the dependency inventory
(`dependencies.rs` spec + `install_optional` in `install.sh` — Ubuntu ships
it, Debian hosts without it fall through to ifupdown or stay temporary-only);
ifupdown deliberately stays write-only (its stanza is boot-applied; an
`ifdown`/`ifup` pair would kill the session — same reasoning, unchanged).

Tests: `src/network.rs` unit **43 → 52** (9 on the pure verdict — down
interface first, exact static v4, no-v4-for-DHCP / any-lease-suffices,
missing v4 route naming the gateway, multi-default-route needle semantics,
static v6 address then route messages, no-v6-intent never verified).
`tests/network_integration.rs` **23/23** unchanged (by design it never PUTs a
real config). lib full run **917 passed**, all integration suites green.
`cargo fmt` clean, `cargo clippy` clean on the touched files.
**Remaining to test live** (added to the list below): a real host pass of the
reconcile — a static change and a DHCP change through the API, the wire
verified after apply, and one deliberately broken apply (bad gateway) watching
the rollback put the previous state back — plus the E2E `network.spec.ts`
re-run against the rebuilt binary.

**2026-10-20 — (a) the Terminal suite re-run against a production bundle after the 2026-10-19
dependency refresh; (b) Network config persistence implemented.**

**(a) Terminal.** The 2026-10-19 dep refresh (`f1fd451`, npm `@xterm/xterm`
**5 → 6**) was never re-run against the Terminal suite. The four failures
recorded on 2026-09-16/17 ("proven pre-existing by `git stash`") rebuild into
exactly two causes — a dev-server artifact and a fresh-instance modal — and one
new regression from the refresh itself:

1. **The whole suite is provided the once-per-version "What's New" modal on a
   fresh scratch instance** (`WhatsNewModal` shows before any
   acknowledgement), whose dialog overlay intercepts the dock click —
   4.2 / 10.3 / 14.2 / 19.1 / 19.2 all timed out on
   `.hv-dock-icon-terminal`. `terminal.spec.ts` now carries the same
   `dismissWhatsNew` helper every newer spec (`tasks`, `metrics`,
   `settings/power`, `settings/mcp`, `snapshot-schedules`) already has.
2. **4.2 (monospace) and 10.3 ("< 80 resource requests") were Vite-dev-server
   artifacts**, exactly as the 09-17 note suspected: against the production
   bundle (scratch server on `:8099`, isolated `DATA_DIRECTORY`, `e2eadmin`
   seeded via `POST /api/auth/setup`) both pass. 4.2 additionally depended on
   the modal being gone.
3. **2.10 ("terminal body paints the same colour as the terminal itself")
   was a real break by the v6 refresh**: in xterm 5 the painted surface was
   `.xterm-viewport`, styled inline with `theme.background` — the test's
   reference. In v6 (new DOM renderer + VS Code-style scrollable element)
   `.xterm-viewport` is a lib-internal layer hard-coded to `#000` in
   `xterm.css`, fully covered by the new **`.xterm-scrollable-element`**, which
   carries the inline theme background. The test now compares the body against
   that surface (same "live against what actually paints" semantics). Verified
   by a DOM probe: `viewport = rgb(0,0,0)` (CSS), scrollable element =
   `rgb(255,255,255)` inline = theme — no visual black band anywhere.
4. **14.1 (rapid add/close) had an overreaching invariant**: it fired ten
   deliberately un-awaited clicks and then demanded exactly `toHaveCount(1)` —
   that tests the scheduler, not the app (observed leftover = 2 tabs, titles
   intact). It now asserts the chaos invariants: page alive, 1..=MAX_TABS
   tabs, strip titles = tab count. **2.8** gained a 400 ms settle before
   measuring geometry (window open/restore animation can flip the
   relative-order assertions mid-transition).

Result: **`terminal.spec.ts` 81 passed / 0 failed on BOTH firefox and
chromium** against the production bundle, scratch server `PORT=8099`,
isolated `DATA_DIRECTORY`.

**(b) Network — `PUT /network/interfaces/{name}/config` now persists.**
`apply_interface_config` only changed the live state, so a reboot lost every
static address / gateway / DNS the admin had applied (the "netplan/interfaces
persistence = deliberate follow-up" of row 14). New `persist_interface_config`
(`src/network.rs`), called by the handler AFTER the live apply succeeds:
backend 1 = netplan (`/etc/netplan/99-homevault-<name>.yaml`, renderer
*mirrored* from the existing files — never invented; static v4 + static v6
share one `addresses:` list and one `routes:` table, since duplicate YAML keys
are a netplan parse error; mode 600 via `sudo tee`), backend 2 = ifupdown
(`interfaces.d/homevault-<name>`, only when `/etc/network/interfaces` sources
the include dir). With neither, the request still succeeds — the live state is
applied — but answers `"(temporary — persistence unavailable)"`, and
`Network.svelte` surfaces the new `network.temp_config` warning (i18n ×4
locales). The persisted config is written, **never applied** (`netplan apply`
would tear the interface down under the admin's own session; runtime and boot
state may differ for one re-apply cycle). No secrets are written (the Wi-Fi
PSK stays in wpa_supplicant's store). *(Closed 2026-10-21: the persisted
config is now applied, verified and rolled back within the same request — see
the 2026-10-21 addendum above.)*

Tests: `src/network.rs` unit **32 → 43** (11 new — netplan DHCP-only shape, the
merged v4+v6 lists/table/renderer, declared-renderer mirroring, bare identifier
key, `netplan_renderer` last-declared-wins / unknown-ignored / empty-dir, file
name namespacing, and the ifupdown `inet static` / `inet dhcp` / `inet6
static|auto|disabled` stanzas; `netplan_renderer` is fs-injected and tested
against a tempdir). `tests/network_integration.rs` **23/23** unchanged (its
matrix never PUTs a real config, by design — it would mutate the host). E2E
`network.spec.ts` re-run green on both browsers against the rebuilt binary
(84 passed / 4 conditionally skipped). `cargo fmt` clean, `cargo clippy` clean
on the touched files, `npm run check` 0 errors, `npm run build` OK.
Docs: `docs/guides/network.md` (new "Persistence" section), `CLAUDE.md`
(2026-10-20 addendum), this row and row 12 updated.

**(c) four HEAD-red lib tests fixed — the 2026-10-19 "20 findings" pass was
written but never executed.** The RAID hardening entry of 2026-10-19 ends
"Not re-run in this session"; running `cargo test` at HEAD proves it: 4 unit
tests fail with no local changes at all. All four now pass, without loosening
a single assertion — the *code* was brought to what its own tests specify:

1. `disk.rs::validate_virtual_disk_request` was reordered: the raw
   `members.len() < min` floor ran first and shadowed both the degraded-create
   semantics (a 2-member raid5 `allow_missing` MUST be valid — the placeholder
   fills to the exact minimum; only a member count that can never reach the
   minimum, budget exceeded, is refused with "cannot start degraded") and the
   sharper raid10 diagnostics (a 3-member raid10 now says "pairs of devices" /
   "even number" instead of the generic "requires at least 4"). The even-count
   rules now measure `md_member_slots(req)` (identical for normal creates; a
   degraded raid10 is still refused by the level's zero missing-budget). The
   ZFS/Btrfs native rejection keeps precedence over the floor, so a
   `zfs + allow_missing` request still gets its specific message.
2. `health.rs`: `run_all_checks` had grown the 9th check `raid_arrays` on
   2026-10-19, but its order test still asserted the EIGHT-check list. Renamed
   to `test_run_all_checks_returns_the_nine_checks_in_order` with
   `raid_arrays` inserted after `disk_smart` (where `run_all_checks` emits it).
   *(Closed 2026-10-22, ROADMAP-1: when `run_all_checks` went async +
   DB-backed the test moved to `tests/health_integration.rs` as
   `test_all_checks_run_in_order_authentication_first` — the assertion is
   unchanged, only the homes changed; see the 2026-10-22 addendum.)*
3. `dependencies.rs`: the same 2026-10-19 pass added two best-effort
   `Command::new` calls (`update-initramfs`, `dracut` fallback) in the mdadm
   post-create path, and the completeness audit
   (`test_every_commanded_binary_has_an_install_source`) rightly flagged them
   as executed-but-uninstallable. They are now pinned in a documented
   `BEST_EFFORT_INITRAMFS_TOOLS` list with the reasoning: the generators are
   both wrapped in `let _ =`, neither may be force-installed (adding `dracut`
   to an apt host risks switching its kernel initramfs regime), and every
   bootable base already ships exactly one of the two.

Counts: `cargo test` full run **green** — lib **892** (was 888 red-of-4),
all 53 integration suites pass, no failure left. `cargo fmt` clean (also
committed: fmt-only whitespace on `disk.rs`/`health.rs`/`handlers/storage.rs`
that predated this session), `cargo clippy` clean on the touched files.

## Summary

| Level | Coverage |
|---|---|
| Manual live audit (CLAUDE.md, invariants) | **29 / 29** screens (Quotas audited live on 2026-10-07; Scheduled Tasks and Metrics live-audited on 2026-10-13) — the Power card and the MCP gateway card live inside Settings (26) |
| At least one relevant Rust unit test | **29 / 29** (Widgets and Quotas: not applicable — Quotas has no code of its own, it reads an existing endpoint; Desktop/shell covered since 2026-09-09 by `src/sse/progress.rs` + `src/handlers/activity.rs` — taskbar) |
| Rust integration test (`tests/*.rs`) | **81 files** `*_integration.rs` (56 + `auth_guard_integration.rs`, the auth-guard contract suite, 2026-10-24 — ROADMAP X.7/X.8, + `acme_integration.rs`, `ddns_integration.rs` and `identity_integration.rs`, the tranche T1 reachability seams, 2026-10-29; Phase 3 added `traffic_control_integration.rs`, `iscsi_integration.rs`, `directory_integration.rs` and `oidc_integration.rs`, 2026-11-01); one per screen except Quotas — Login, Setup Wizard, Account, Docker (+ `compose_integration.rs` — editing the compose of a container, + `docker_integration.rs` — volumes, prune, exec, inspect, live-log tickets, builds, registries, daemon config), Users (+ `quota_integration.rs` — the truthful home-quota contract of `GET /users/{u}/home`, 2026-10-22), Groups, Rescue, Network, VPN, Shares/Protocols, Settings (security cards **+ core** — `settings_core_integration.rs` **+ built-in HTTPS/TLS — `tls_integration.rs` + certificate automation — `acme_integration.rs` + dynamic DNS — `ddns_integration.rs`**), Downloader, Storage, RAID, Backup, Cloud Sync, Syncthing, Terminal, Chat (+ `mcp_integration.rs` — external-agent server + catalog phase-B endpoints, + **`rag_integration.rs`** — the knowledge base index/search/reconcile contract, 2026-10-22), Explorer, Dashboard, System journal, Widgets, Desktop/shell (+ `activity_integration.rs` — taskbar), Health, **VirtualMachines**, **Scheduled Tasks**, **Metrics**, Power; **the Quotas app: no file of its own** — its only endpoint (`GET /transfer-quotas/{type}`) is covered by `transfer_quota.rs` + `explorer_integration.rs`. The remaining files are cross-cutting (`mdns`, `notifications`, `progress`, `snmp`, `ups`, `weather_location`, `app_catalog`, `app_proxy`, `compose`, `security`, `tls`, `share_links`, `spa_assets`, …) |
| Playwright E2E test (`tests/e2e/`) | **36 screen specs out of 61 files** — Login (+ `password-change.spec.ts`, the forced first-sign-in change screen), Setup Wizard, Account (+ `account/identity.spec.ts`, trusted devices, rescue e-mail and the sign-in recovery route, 2026-10-29), Users (+ `users/admin-sessions.spec.ts`, `users/account-expiry.spec.ts` and `users/group-access.spec.ts`, the per-user sessions, account-expiry and group-grant sub-specs, 2026-10-20), Groups (+ `groups/group-identity.spec.ts`, the rename + description sub-spec, 2026-10-20), Terminal, Settings (+ `settings/power.spec.ts`, `settings/mcp.spec.ts`, `settings/knowledge-base.spec.ts` — the RAG knowledge base, 2026-10-22, + `settings/acme.spec.ts` — certificate automation, 2026-10-29, + `settings/ddns.spec.ts` — dynamic DNS, 2026-10-29), RAID, Docker, Rescue, Network, VPN, Shares/Protocols, Downloader, Storage (+ `storage/snapshot-schedules.spec.ts`), Backup, Cloud Sync, Syncthing, Chat, Explorer, Dashboard, System journal, Widgets, Edit widgets, Desktop/shell (+ `shortcuts.spec.ts`, `power.spec.ts` — the mocked restart/shutdown feedback, 2026-10-24), Health, **VirtualMachines**, **Quotas**, **Scheduled Tasks**, **Metrics**, **Directory sign-in** (+ `directory/directory.spec.ts`, all three providers, 2026-11-02), plus `security/` (sub-feature, not a section of `SCREENS.md`), plus **`design/typography.spec.ts`** (cross-cutting: native text-ladder conformance + role table + the 11px floor + text scaling, walked over the login screen and five apps); the Quotas spec (`tests/e2e/quotas/quotas.spec.ts`, written on 2026-09-22) has been **green end to end since 2026-10-07** — **38/38** (Firefox, scratch server) — see the per-screen detail line 6 |

All 29 screens have been verified by hand **and** have at least one
automated test (Rust or Playwright) — none is without a non-regression
net (VirtualMachines: verified live against a real Incus —
containers, Linux ISO VM + noVNC screen, concurrent creation, ISO
library; only the boot of a real Windows ISO remains to be confirmed).

**2026-10-13 (c) — OMV-inspired priority-1 features.** Five features shipped
together, each with its full test stack (Rust unit + integration, Playwright
E2E on a scratch instance, docs + RAG FeatureDocs): **Scheduled Tasks** (new
app, rows 28 + `tests/scheduled_tasks_integration.rs`), **Metrics** (new app,
row 29 + `tests/metrics_integration.rs`), **host Power management + disk
spindown** (Settings card + `tests/power_integration.rs` +
`settings/power.spec.ts`), **per-event notification toggles + webhook sink
formats** (`notification_events.rs` + 4 new `notifications_integration.rs`
tests) and **snapshot schedules with GFS retention** (Storage → Snapshots tab
+ `snapshot_schedule_integration.rs` + `storage/snapshot-schedules.spec.ts`).
Migrations 043-046. Totals after this pass: **1 492 Rust tests** (lib +
integration, 0 failed), **clippy clean of new warnings** (the 6 remaining are
pre-existing: pools.rs tabs-in-doc, auth.rs let-binding,
whats_new_integration.rs len-zero), `npm run check` 0 errors, `npm run build`
OK. Two latent frontend bugs found and fixed in the same pass: `ttl: 0` in
`api.ts` is falsy and silently fell back to the 5 s default TTL (5 endpoints
affected, including the topbar notifications — now `cache: false`), and the
per-task icon actions were 18 px-wide buttons whose 44 px touch pads
overlapped (now the house `w-9 h-9 grid place-items-center` sizing).

**2026-10-19 — the RAID creation feature was hardened and wired to the background-job system (20 numbered findings fixed in one pass).** Creation moved from a synchronous request to a **tracked progress job**: `POST /api/storage/raid` now validates shape + devices + name-collision synchronously (clean 400s), answers `{message, job_id}` immediately, then runs mdadm/ZFS/Btrfs behind a single-flight `CREATE_LOCK` with progress milestones on the existing SSE (`JobType::Custom("raid_create")`); a failed job emits a `failed` event and never a `done` one. The mdadm arm gained `--name` (stable metadata naming), `--bitmap=internal` on redundant levels, hot spares (`--spare-devices`) and deliberate degraded creates (`missing` slots up to the level's budget: raid1/5→1, raid6→2), replaced the blind 2 s sleep by `/sys/block/mdX/md/array_state` polling with resync-percent reporting, and tears down its own half-built array (stop + zero superblocks, logged) when format/mount fail. fstab persistence is now serialized behind `PERSIST_LOCK`, verified with `sudo findmnt --verify --fstab` (rolled back + failed on a malformed line — verified grants added to the scoped sudoers), and followed by a best-effort initramfs refresh (`update-initramfs -u`, `dracut -f` fallback). The ZFS arm mounts under `RAID_MOUNT_BASE/<name>`, applies the shared dataset policy (`compression=lz4`, `atime=off`, `acltype=posixacl`, `xattr=sa`, mirrored into `pools.rs`), and raid10 is now a **stripe of mirrors** (was mis-mapped to raidz3). Everything dispatches through one `create_virtual_disk`. `get_available_devices` lists **every** non-system disk with a facts-based `reason` when busy and a `smartctl -H` `smart_status`, scanning the whole tree in **one** `lsblk -P` pass (no more per-disk shell storm); tool failures surface as `DiskError::CommandFailed` through `friendly_tool_error` (busy/too-small/in-use/permission patterns + the tool's last error line). New: `POST /api/storage/virtual-disks/{name}/grow` (mdadm `--add` + `--grow`, raid1/5/6 only, device gate like creation) with a "Add disk" modal in Storage, replacement-disk gate on rebuild, and a 9th health check `raid_arrays` (degraded→Warning, failed→Error) with its notification-catalog entry. Frontend: the wizard row list shows in-use rows dimmed + reason + SMART chips, offers spares beyond the level minimum and degraded creates below it, follows the job on SSE (progress bar locks the footer), and warns about raw capacity wasted by unequal disk sizes; i18n keys added ×4 locales (en/fr/de/es). Docs: `docs/guides/raid.md`, `docs/API.md`, this file. Tests: `disk.rs` **22 → 25** unit (request validation, friendly errors, sub-progress; `create_raid` tests moved to the `MdadmRaidRequest` signature), `raid_integration.rs` 5 (grow endpoint in the router + write matrix), `storage_integration.rs` 28 (same), `health_integration.rs` 10 (constants 8 → 9), `health.rs` 13 unit (dispatch list extended); `tests/e2e/raid/raid.spec.ts` 13.1/13.3 now poll the virtual-disks list (creation is asynchronous), `tests/e2e/health/health.spec.ts` `EXPECTED_CHECKS` 8 → 9; `cargo check` + full `cargo test --no-run` compile clean, `npm run check` 0 errors, `npm run build` OK. **Not re-run in this session** (per instruction): no unit/integration/E2E test was executed; the opt-in RAID lifecycle and the Health specs were edited for coherence, not validated live. **(Closed 2026-10-20: the first real `cargo test` run found 4 of these unit tests red at HEAD — 2 in `disk.rs` (degraded-create bounds, uneven raid10), 1 in `health.rs` (8→9 checks order), 1 in `dependencies.rs` (the new initramfs fallbacks) — and the production code, not the assertions, was brought in line; see the 2026-10-20 addendum, part (c).)**

**2026-10-22 — Health: ROADMAP item 1 (P0) — the 5 findings fixed in one pass
(1.1–1.5).** (1.1) `HealthService::run_all_checks`/`run_check` are now
`async` and self-offload the eight system checks to `spawn_blocking`
*inside* the service (`check_authentication` alone awaits the pool on the
runtime, as a DB read should) — callers simply `.await`; the notification
engine's re-wrap was removed and both handlers updated. A hung `smartctl`
can slow one answer but can never park a Tokio worker. (1.2) every
`smartctl` invocation in the tree now goes through one shared helper
(`src/smart.rs`, `sudo smartctl <args> <device>`; std + tokio variants):
Health's `sudo smartctl -H`, Dashboard's `run_smartctl` (USB `-d sat`
retry kept) and `get_drive_temperatures`, Storage/disk.rs's 8 call sites
(`get_smart_details`, `-H` overall health, temperature, RAID-wizard
`get_smart_status`, `get_smart_capabilities`, `start_smart_test`,
self-test status + remaining — the bare unprivileged ones silently saw
nothing on `660 root:disk` devices) and the Scheduled-Tasks SMART-test
action. (1.3) `check_authentication` performs a real account-table read
(`SELECT COUNT(*) FROM users`, scalar only — the endpoint is
household-readable); a corrupt/unreachable DB now surfaces as `unknown`
instead of a hardcoded "operational"; `check_systemd_services` adds the
appliance's **own unit parsed from `/proc/self/cgroup`** (derived, not
hardcoded, and skipped when not running under systemd) whose failure is
`Unhealthy` while another monitored service failing stays `Warning`. (1.4)
`check_network_interfaces` takes the link state from
`/sys/class/net/<iface>/operstate` (`lo` special-cased as up; `operstate`
echoed in the details) instead of inferring "up" from idle rx/tx
counters — an idle LAN port no longer reports a healthy appliance as
`warning`. (1.5) per-check partial degradation: any check error becomes a
well-formed `Unknown` slot ("Check unavailable: …") via `HealthService::degrade`
while the other eight still run and `/api/health` keeps answering
`200 success:true`; the frontend renders an explicit error state
(`data-testid="health-error"`) with a Retry button (`health-retry`,
`health.retry`) instead of the silent empty state. i18n keys
(`error_title`/`error_desc`/`retry`) added ×4 locales. Docs:
`docs/guides/health.md` (new "Per-check degradation contract" section +
per-check contracts + invariants), `docs/API.md`. Tests: `src/health.rs`
**13 → 17** unit (three DB-dependent tests moved to the integration
file below; 8 new pure ones: degrade passthrough/degradation,
`link_is_up` operstate matrix, `parse_cgroup_unit`,
`service_names` with the derived own unit, loopback + operstate details),
**`src/smart.rs`
2 unit** (argv shape, std = tokio), `tests/health_integration.rs`
**10 → 15** (order with `authentication` first, every `run_check` arm
against a real pool, real account-table read, parked-`users` degradation
→ `unknown` with the eight others still running + endpoint still
`success:true`, same through the mounted handler),
`tests/e2e/health/health.spec.ts` **35 `test(`** (11.1 rewritten to the
error state + retry, new 11.3 retry-against-real-backend recovery). **All
verified live in the same session**: full `cargo test` green
(lib **1073** tests + all 53 integration files, 0 failed),
`cargo fmt`/`cargo clippy` clean on the touched files, `npm run check` 0
errors, `npm run build` OK, and the Health E2E spec re-run green
**37/37 on Firefox and 37/37 on Chromium** against the rebuilt binary on a
scratch server (`PORT=8099`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded
via `POST /api/auth/setup`).

**2026-10-22 — ROADMAP item 10 (File Explorer): the four blocking findings
fixed with their tests in the same change.** (10.1) Extraction is
zip-slip-safe: members listed (in the C locale — French "lien vers…" breaks
the ` -> ` parse) and validated pre-flight, absolute/`..`/control-char names
refused, symlink-member **archives refused**, extraction into a fresh
`.hvextract-<uuid>` staging dir inside the destination, the staged tree
**verified confined** (canonical walk incl. symlink resolution) then merged
with the historical overwrite semantics, staging removed on any failure; the
`.gz` branch streams `gunzip` stdout **straight to disk** (was a full
RAM buffer) and deletes a partial on failure. (10.2) The SSE watcher is
authorised: `POST /explorer/watch/ticket` takes `{path}`, validates it
against the caller's roots, and the ticket is bound to **(user, path)**
(single-use, TTL 15 s); a path mismatch is refused — **without** burning the
ticket, so a stale UI retry still works; events are filtered
component-based (`Path::starts_with`) so `/media/raid2` can't leak into a
`/media/raid` watch; a `WatchGuard` RAII-releases the inotify watch on
stream drop (the leak that eventually exhausted `max_user_watches`). (10.3)
Resumable upload sessions are owned — chunk/complete/abort compare the
caller to `session.username` (403) — and capped per user (20) and globally
(256) with a 429 on the 21st slot. (10.4) `POST /explorer/delete` runs its
path through the **PathGuard even for admins** and refuses to delete a
**browsing root itself**; the validated path (not the raw query string)
reaches `delete_item`. Also in the pass: `compress_items` passes `--`
before member names (no argv injection through `-`-named files), and the
eleven silent `else {}` failure branches in `Explorer.svelte` (plus partial
batch failures on delete/copy/move/upload) now report visibly via
`dlg.notify`, with 19 new i18n keys ×4 locales. Tests:
`src/handlers/explorer.rs` unit **17 → 33** (member-name/link-target
validators, tar/zip/7z listing parses, staging-confinement walker, staged
merge/replace semantics), `src/sse/file_watcher.rs` unit **0 → 4** (ticket
path binding incl. prefix-sibling refusal, redeemable after refused
attempts, single-use, unknown/empty),
`tests/explorer_integration.rs` **55 → 64** (traversal tar + traversal zip +
symlink zip refused with nothing extracted — evil archives crafted by hand
because GNU tar sanitises member names on the way in; `.gz` streaming
round-trip; admin delete confined + root refusal + inside-root still ok;
watch ticket bound/single-use/streams; upload ownership end-to-end;
per-user cap), `compress` dash-name regression. **All verified in the same
change**: full `cargo test` green (lib **1088** tests + all 53 integration
files, 0 failed), `cargo fmt`/`clippy` clean on touched files,
`npm run check` 0 errors, `npm run build` OK. Explorer E2E count unchanged
(64 at `--list`): the four contracts are HTTP/`oneshot`-level, the browser
side has no new state to pin.

**2026-10-22 — ROADMAP item 14 (Account / Security): the two blocking
findings fixed, plus the avatar leftovers confirmed and removed.** (14.1)
`GET /security/sessions` no longer maps `token: s.token.clone()` into the
payload — every session is identified by `SessionInfo.id`, a 12-char SHA-256
fingerprint (`session_public_id`) that cannot be replayed as a credential.
(14.2) `POST /security/sessions/revoke` now takes `{ ids: [] }` and resolves
each id **only against `find_all_by_username(caller)`**, so a token or id
belonging to another account is not even a candidate: it answers 404
("No matching session to revoke for your account") and leaves the other
account signed in (the previous code passed the request's raw tokens to
`delete_many`, a cross-account DoS primitive). The current session stays
filtered out, `revoke-all` was already caller-scoped and is now pinned by a
test, and an empty `ids` array is a 400. Frontend: `Account.svelte` keys,
renders and revokes on `s.id`; `api.ts::revokeSessions(ids)` posts `{ ids }`.
Also in the pass: `render_user_avatar` / `missing_avatar_response` had **zero
callers** (confirmed by grep) and are deleted — the seed-storage +
`GET /users/{username}/avatar.svg` path is the whole feature; the public
`/avatars/preview` route is a documented deliberate exception (setup wizard,
pre-auth, renders from a client seed, touches no user data) and
`docs/guides/avatars.md` no longer claims an unconditional "cache" (the cache
header is `public, max-age=300, immutable` only with a persisted seed;
`?fresh=true` and a seed-less user answer `no-store`). Docs:
`docs/guides/security.md` (endpoint table — `login-history/{username}` is
admin **or self**, matching `handle_get_user_login_history` — plus a new
"Session identifiers" section and the invariants), `docs/guides/account.md`
(new sessions + recent-sign-ins sections). Tests: `handlers/security.rs`
unit **0 → 4** (`session_public_id` determinism, 12-hex shape, differs across
tokens and on a single-character change), `account_integration.rs`
**25 → 30** (payload carries no live token; a foreign token/id is a 404 with
the other account still signed in; the current session is refused and a mixed
list revokes only the others; empty `ids` → 400; revoke-all stays
caller-scoped), `account.spec.ts` **74 → 75** (14.6: opaque ids in the payload
and the DOM, revoke-by-id, foreign token → 404). The E2E number is the one
`playwright test --list` reports; the file's 73 `test()` declarations are not
the same thing because the `10.x` responsive loop generates three tests from
one declaration (the old "72" in this table was that grep count — corrected
here). Verified end-to-end on a scratch production-bundle instance
(`PORT=8099`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via
`POST /api/auth/setup`): 14.6 **passes on Firefox and Chromium**. The full
`cargo test` run is green (lib + all 53 integration suites + doc-tests).
Counts unchanged elsewhere. Local checks: `cargo test --lib handlers::security`
4/4, `cargo test --test account_integration` 30/30, `cargo fmt`/`clippy` clean on
touched files,
`npm run check` 0 errors, `npm run build` OK.

**2026-10-22 — ROADMAP item 13.1 (Settings): the update check was a third-party
call nobody asked for, and is now opt-in and enforced server-side.** Opening
Settings ran `loadVersionInfo()` unconditionally in `onMount` →
`GET /settings/version` → `fetch_latest_version()`, so every visit to the screen
contacted `api.github.com` whatever `UpdateFrequency` said; `Manual` and
`Disabled` were decorative, and `should_check_for_updates()` /
`CACHED_UPDATE_RESULT` had zero readers. A privacy-baseline violation (CLAUDE.md
→ Compliance, point 1). Fix: `is_check_due(frequency, last_check, now)` is the
single pure gate — no interval (`Manual`, `Disabled`, absent) never fires, an
opted-in schedule fires once its interval elapsed; `GET /settings/version`
serves the cached answer (refreshed against the running version) unless a check
is due, and the new admin-only `POST /settings/version/check` is the explicit
"Check now". The frontend's button now calls that POST (the mount keeps the
read-only GET) and the card says so when no automatic check is scheduled.
`update_check_attempts()` counts attempts (incremented *before* the I/O, so a
failure is still an attempt) and `HOMEVAULT_VERSION_CHECK_URL` redirects the
endpoint — that is how the integration tests prove "zero outbound attempts"
against a local listener instead of assuming it. Tests: `handlers/settings.rs`
unit **13 → 15** (`is_check_due` matrix), `settings_core_integration.rs`
**13 → 18**, `settings.spec.ts` **103 → 104** (8.3 now asserts the POST, 8.4
asserts the mount forces nothing). The same pass fixed a **pre-existing spec
bug**: `settings.spec.ts` never dismissed the once-per-version "What's New"
modal, so on a fresh scratch instance the *first* test of every run timed out on
`settings-page` (8.1 and 7.1 both reproduced it, green as the second test);
`gotoDesktop` now carries the same `dismissWhatsNew` helper as the
power/mcp/metrics/account specs. Docs: `docs/guides/settings.md` (endpoint
table + a new invariant) and `docs/API.md`. Verified: `cargo test --lib
handlers::settings` 15/15, `cargo test --test settings_core_integration` 18/18,
`settings.spec.ts` **104/104 green** (Firefox, scratch `:8099`), `npm run
check` 0 errors, `npm run build` OK, `cargo fmt` clean. **The whole `cargo test`
suite is green after the change** (57 binaries, 0 failure — `account_integration`
30/30 and `settings_core_integration` 18/18 included, so nothing else depended on
the old "always fetch" behaviour of `/settings/version`).

**2026-10-22 — ROADMAP item 4 (AI Assistant / RAG, P0): the knowledge base gained
its user-facing surface and an integrity contract.** (4.1) Nothing in the product
called `/api/rag/*`: `api.ts` carried an empty `// RAG Knowledge Base` comment
block and no control existed anywhere, so the index stayed empty and the assistant
answered "I don't know" for shipped features. New
`KnowledgeBaseCard.svelte` (Settings → AI Assistant) shows the index status
(documents, chunks, `idle`/`indexing`/`error`, last indexed), installs the
embedding model and triggers a reindex, polling `GET /rag/status` every 2 s while
`indexing`; the Chat empty state shows a notice (admin: "index now") when the index
has zero documents. `GET /rag/status` now always reports the model the index needs
(`nomic-embed-text`), even before the first index, so the card can tell whether it
is installed. The card reuses the dead `chat.rag_*` keys the retired Chat RAG
panel left behind (5 unused search-tester keys removed, 9 added, ×4 locales).
(4.2) Reindex reconciles: each indexer returns the ids it produced and
`rag::prune_source` deletes every document of that source the run did not —
`clear_source` (zero callers before this) is now its empty-keep-set case. An
embedding failure fails the run loudly: `upsert_document_with_chunks` embeds before
writing and returns `Err`, so no chunk is ever stored with a NULL embedding (the old
`.unwrap_or_default()`); `index_all` records `error` and returns `Err` instead of
warn-and-continue. `last_indexed_at` and the counts now only advance on a completed
run, so a failed reindex no longer reports "last indexed: now" with zero documents
while the previous index is still answering. A reindex still runs in the background job envelope, and a
second request while one is in flight is refused (`ReindexGuard`, answer
`success:false`). `GET /rag/documents` no longer turns a DB error into an empty
list, and the search handler still distinguishes an error from an empty result.
The indexers are generic over a new `rag::EmbeddingProvider` (`OllamaEmbedder` in
production, a deterministic fake in tests), so all of this is testable offline.
(4.3) `handle_chat_models_recommend` offloads `detect_system_specs` (which shells
out to `nvidia-smi`/`rocm-smi` and reads `/proc/meminfo`) to `spawn_blocking`; the
Chat RAG search failure is logged instead of swallowed. Also on 2026-10-22: the
`/chat/*` error strings and the pull success message are **English** — they leaked
French into a German/Spanish session (the model recommendations' `description`/
`speed` are still French-only, recorded as a known gap in
`docs/guides/chat.md`). Docs: `docs/guides/rag.md`,
`docs/guides/chat.md`, `docs/API.md`. Tests: `rag.rs` **10 → 15** unit
(prune/clear, embedding fail-loud leaves nothing behind, chunks carry a real
embedding), `rag_indexers.rs` **3 → 5** unit (`index_all` reconciles a stale
document away; an embedding failure returns `Err`, sets `error`, stores nothing),
**`tests/rag_integration.rs` (8, new)** — HTTP auth boundaries (status
session-level, index/search/documents admin), search error-vs-empty, and
index → search → reconcile → remove through the provider seam;
`tests/e2e/settings/knowledge-base.spec.ts` (4, new). Verified: `cargo test --lib
rag` + `rag_indexers` green, `cargo test --test rag_integration` 8/8, `npm run
check` 0 errors, `npm run build` OK, and the E2E spec **4/4 green on Firefox
against a live scratch server** (`scripts/dev-instance.sh dev-1 8181`). The same
live server confirmed the fail-loud path end to end: `POST /api/rag/index` on a
host without Ollama ends with `status: "error"`, `last_indexed_at: null` and the
previous counts intact (no `NULL`-embedding chunks).

**2026-10-23 — ROADMAP item 16 (Cloud Sync / rclone): every subprocess is bounded,
mounts are owned, and the credential file is private.** (16.1) `src/utils/process.rs`
gained `run_timed_async_result` + a named `ProcessFailure { Spawn { program, error },
Timeout { program, deadline } }` — "not installed" and "too slow" now read differently
instead of collapsing into one `io::Error`; `run_timed_async` delegates to it and keeps
its `.ok()` shape (5 new `#[tokio::test]`: fast command, non-zero exit kept as data, a
timeout that names its deadline, spawn-vs-timeout distinction, the wrapper). The four
rclone deadlines (`RCLONE_PROBE_TIMEOUT` 10 s, `RCLONE_MOUNT_READY_TIMEOUT` 20 s,
`RCLONE_UMOUNT_TIMEOUT` 30 s, `RCLONE_TRANSFER_TIMEOUT` 6 h) replace every bare
`.output()` in the module — `rclone version`/`listremotes`, `fusermount -V`/`-u`,
`umount -f`, and `sync`/`copy`/`jobs/{id}/run`, whose `cloud_sync` activity job is now
completed or failed-with-reason on **every** branch (a stalled remote used to hold the
HTTP request and the job open forever). (16.2) `handle_mount_start` dropped `--daemon`,
keeps the `Child` (`kill_on_drop(false)`) in a process-wide registry (`static MOUNTS`,
keyed by mount point — a static so the 40+ `AppState` literals in `tests/` did not have
to grow a field), drains stderr into a bounded 4 KiB tail for the failure message, and
returns success only once `wait_until_mounted` sees the mount point in
`/proc/self/mounts`; `handle_list_mounts` reads the same table (whole-field match,
`\040` decoding), enriches it from the registry, and reports a tracked child whose mount
disappeared as `is_active: false`; `handle_mount_stop` (`MountStopRequest`) reports both
`fusermount` and `umount` reasons; `shutdown_mounts()` runs from `shutdown_signal` in
`src/web/mod.rs`, so a restart no longer leaves stale FUSE mounts and orphaned `rclone`
processes. (16.3) `write_private_config` writes the config **and** its dated backup 0600
(open `.mode()` + `set_permissions` repair; the old `tokio::fs::copy` recreated the
backup 0644), section checks/removals are parsed (`config_has_section` matches a whole
trimmed header line, `remove_section` ends at the next line *starting* with `[`, so
deleting `my` no longer touches `[mydrive]` and `patterns = [a-z]` no longer truncates a
section), `add_remote` validates the remote name and the provider slug (`type = {provider}`
is written verbatim — a newline there was the same INI injection as the free-text field),
mount points are confined to `RCLONE_MOUNT_BASE` as `validate_mount_point` (strict child,
no `.`/`..`, no control character, no leading `-`) and `GET /rclone/status` returns the
`mount_base` so the screen stops hardcoding `/mnt/rclone/<name>`. Residual defects fixed:
`/mount/stop` was a **422** for the `{ mount_point }` the UI sends (dedicated request
struct), and the screen swallowed every failure — seven empty `else {}`
blocks (add/delete remote, mount, unmount, sync, save job, save config), two silent
job handlers, and five loaders that dropped their error. One dismissible page-level
alert (`data-testid="cloudsync-error"`) now shows the server's reason for all of them,
and a failed jobs refresh keeps the list instead of blanking it. The sync selects also offer `local`-type remotes again (`build_rclone_path`
supports them; the filter was screen-only). Left open on purpose, as its own P2 item
**16.4**: there is still no scheduler — `POST /rclone/jobs/{id}/run` is the only way a
persisted job runs, and the screen's "last run/status" is the outcome of the click, not
a server-side run state. Docs: `docs/guides/cloud-sync.md` (deadline table, mount
lifecycle, config hardening, invariants, known debt), `docs/API.md`, `ROADMAP.md`.
Tests: `handlers/rclone.rs` **15 → 39** unit, `rclone_integration.rs` **17 → 26**,
`cloudsync.spec.ts` **24 → 29** runtime tests. Verified: `cargo check --all-targets`
clean, every Rust file this pass touched is `rustfmt --check`-clean (`cargo fmt
--check` still reports untouched modules that were already unformatted at `HEAD`),
`cd frontend && npm run check` 0 errors / 0 warnings and `npm run build` OK. `cargo clippy --all-targets -- -D warnings` reports **no lint in any file
this pass touched**; the 18 that remain are pre-existing on this toolchain (clippy
1.99.1, no toolchain pin) in untouched modules — `disk.rs` 13, `dependencies.rs` 3,
`utils/proc.rs` 1, `firewall.rs` 1. **No test suite was run in this pass** (explicit request);
the mount-ready poll, the shutdown unmount and the 0600 repair are covered by the new
tests above, not by a live FUSE mount.

## Detail by screen

| # | Screen | Manual audit (CLAUDE.md) | Rust unit tests | Rust integration tests | Playwright E2E tests |
|---|---|---|---|---|---|
| 1 | Login | [x] 2026-07-13 (+07-15, 07-17, 09-03, 09-04) ; **+2026-10-17 (c): lockout is temporary again (`locked_until`, migration 047), per-client-IP throttle on login + 2FA, client IP taken from the TCP peer (`middleware::PeerAddr`), and the login screen's error a11y (live region, `aria-invalid`/`aria-describedby`, focus restore)** | [x] `auth/service.rs`, `auth/tests.rs`, `handlers/auth.rs` (`client_ip` — 6), `middleware/password_change_guard.rs` (**5** — 2026-10-20, `must_change_password`: the allowlist + the self-only `PUT /users/{u}/password` path decision as pure tests) | [x] `login_integration.rs` (**61** — **+2026-10-18 (recs): +2 — `test_2fa_verify_accepts_a_previous_window_code` (a code from the PREVIOUS 30 s window completes login: pins the ±1 RFC 6238 window) and `test_2fa_secret_is_encrypted_at_rest` (the `user_2fa` column holds an `hv1:` blob, never the base32, and a code computed from the plaintext still verifies end to end). 60 → 62** — +6 on 2026-10-17 (c): `test_failed_logins_record_a_temporary_lock_expiry`, `test_expired_temporary_lock_lifts_itself_on_login`, `test_unexpired_temporary_lock_still_blocks`, `test_admin_lock_without_expiry_is_permanent`, `test_password_spraying_is_throttled_per_ip`, `test_a_few_failed_logins_are_not_ip_throttled` — 54 → 60; +1 on 2026-10-13: the hardware-change tests were re-pointed from the removed `.setup_complete`/`.system_hash` marker files to the `setup_state` table + an admin row, and a third case was added — an admin with **no** recorded fingerprint is configured, not a first run. 53 before), `login_pam_integration.rs` (9), `auth_recovery_integration.rs` (7), `password_change_integration.rs` (**8** — 2026-10-20: login flags an admin-created member but not a seeded row, `/auth/me` exposes the flag, the guard 403s `password_change_required` on everything but whoami/logout/its own change (another user's password target included), a self change clears the flag, an admin reset re-arms it, an admin's own change does not flag) | [x] `login.spec.ts` (**97 `test(` / 101 runtime** + **§27 Passkey sign-in, 5 tests (2026-10-31 P1.3)** — the button appears when WebAuthn exists and is **absent** when it does not; it refuses to start without a username (no `login/begin` request fires); a cancelled ceremony renders the *cancelled* sentence, never "invalid credentials"; no credential/challenge reaches the URL. Written, **not run** — a headless browser has no authenticator, and the crypto is covered by `passkey_integration.rs` — **+2026-10-18 (recs): re-run after the three hardening recommendations — 101/101 (chromium, fresh scratch server). No test added/removed: the suite's `totpNowSafe` helper is kept as clock-skew robustness now that the backend accepts the ±1 window** ; **+2026-10-18: full re-run on a fresh scratch server (isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup` as in CI) — 101/101 (chromium). The re-run exposed two suite bugs, both fixed in the same pass: (1) the §9 `afterAll` teardown computed its login TOTP with no window-rollover retry — a code built in the last seconds of a 30 s window is rejected by `verify_totp_code` (which accepts the current + next window only), so the teardown's `/auth/2fa/verify` intermittently got no token, skipped the `disable`, and left the seeded admin with 2FA enabled — hard-failing 22.1 later in the file and every rerun (reproduced 1-in-3 on §9-only reruns). All TOTP call sites now go through a `totpNowSafe` helper that waits out the tail of a window before computing. (2) 22.1's skip guard used `isVisible({ timeout })`, which returns immediately (the option is ignored) and so never saw the 2FA step rendering a moment after the submit — replaced with a real `waitFor`, so a 2FA-enabled admin now skips instead of failing. Count re-measured `--list` (per project) 95 → 101; no test added or removed in this change** ; **+2026-10-17 (c): §25 "Lockout hardening" — 25.1 the inline error is an assertive live region + `aria-invalid`/`aria-describedby`, 25.2 closing the error modal returns focus to the form, 25.3 five wrong passwords lock the account and the copy points at the recovery-code path (locale pinned to `en-US`). 92 → 95. Spec not re-run** ; **+2026-10-17 (b): 13.7 added — the gateway corners use the standard stops (setup card `rounded-2xl` 16px / login field `rounded-md` 8px), and the login/setup fields + buttons were moved off `rounded-lg` (10px) onto the control stop; count 91 → 92. Spec not re-run** ; **+2026-10-17: 13.6 rewritten — the gateway palette is now an alias of the Appliance palette** (`--hv-login-accent` === `--hv-primary`, gateway canvas === `--hv-desktop-bg`) and every value that carries ink clears its floor: the accent ladder, the wizard ink ramp, the success/warning families, the white recovery box + its amber code chip, and all three gradients (logo / CTA / create) — read live through a probe element, so an alias is measured as its colour and never as `var(...)` text; count unchanged. **+2026-09-23: 13.6 contrast guard of the Login tokens**: accent / `soft-border` / the 2 gradients ≥4.5:1 under white, read on the live `:root` — see "standardized contrast" l. 26; 90 before) ; **+2026-10-21: production-ready pass — the fake Cancel is real (process-group SIGTERM→SIGKILL, `CancelOutcome` + `ProgressState::Cancelled`), the status endpoint is confined to the rescue directories (no symlink, ≤16 MiB), clones get a safety net (same device / in-use destination / smaller destination, live root disk via `findmnt`), every un-honourable option is a 400 instead of a silent no-op, all subprocesses run under a timeout, the default mapfile left `/tmp` for the root-owned `rescue/` dir and PhotoRec's tree is chown'd to the admin. TestDisk's advanced block was removed (nothing left to put there)** ; **+2026-09-12: Appliance design migration — auth card + 2FA + code recovery in Tailwind-on-tokens (native inputs kept for autofill, native submission chain verified), selectors `data-testid="login-*"`, zero scoped CSS** ; **+2026-09-12: §12 anti-zoom iOS — form fields ≥16px on touch pointer** (`pointer: coarse`, cf. `docs/guides/design-system.md` §5.1) ; **+2026-10-13: the login screen no longer renders the wizard inline — `App.svelte` renders it from `$appGate.route`, and `Login` gained the `recoveryMode` hardware-change banner + `/auth/setup/rebaseline` call and the `connectionError` prop. Spec not re-run** ; **+2026-10-20: forced first-sign-in password change (migration 049, `users.must_change_password`) — the flagged route is `appGate` route `password-change`, rendered by `Login.svelte` in `passwordChangeMode` (current/new/confirm + cancel = real server logout, the §3.1 empty-new case validates client-side with no `required` attribute, native bubbles replaced by the `login-pwchange-error` live region). New spec `tests/e2e/password-change/password-change.spec.ts` (**14 `test(` / 16 runtime**) — **16/16 green (Firefox, scratch server on :8099, admin seeded via `POST /api/auth/setup`)**; it caught two real bugs in the same pass: the guard initially refused `GET /auth/setup-status` (the reload-mid-flow boot read → 403 → the gate died before `/auth/me`, fixed by adding it + `/api/settings/public` to the allowlist), and the first spec draft raced its own fixture across workers (a suite-internal member password changed by §4.1 broke later logins — each mutating flow now uses its own dedicated member). Client-side `npm run check` 0 errors** |
| 2 | Setup Wizard | [x] 2026-07-13 (+07-17, 09-03) ; **+2026-10-13: verified live logic through the Rust suites — first-run state derived from the database, hardware-change recovery, local-network guard** ; **+2026-10-13 (b): the wizard grew from 4 to 8 steps (welcome, language, storage, RAID, restore, account, password, verify) and became reboot-safe — the step + non-secret choices persist server-side** ; **+2026-10-17 (b): the storage, RAID, restore and language steps were removed — the wizard is down to 4 steps (welcome, account, password, verify). RAID is configured from its own app, the OS is script-installed, a backup is restored from the Backup app, and the interface language is now detected from the OS/browser locale (`GET /auth/setup-status` → `system_language`, English when unsupported) instead of being asked; `GET /auth/setup/storage-options` and the frontend `getSetupStorageOptions` were removed, `MAX_SETUP_STEP` dropped 7 → 3, and `setup_progress.data` now carries nothing** | [x] `setup.rs` (**88** — +2 on 2026-10-13: `system_hash_matches` accepts the current hash, rejects a stale/empty one; **+2026-10-17 (c): +5 `language_from_locale`/`detect_system_language`, +6 lock-staleness (`lock_contents_is_live`, `process_is_alive`), +5 password policy (12-char floor, banned list, repeated run) — 45 → 61**; **+2026-10-23 (ROADMAP 7.1): +12 over the rendered Storage grant table (`StorageBins`) — no entry may be a bare binary or reach a shell (`bash`/`sh -c`), `zpool`/`zfs`/`btrfs`/`cryptsetup` are per subcommand, `mkfs.*`/`wipefs`/`blkid` per flag shape, `mount`/`umount`/`mkdir`/`rmdir` confined to the appliance roots, `tee` to `/etc/fstab` + `/etc/mdadm/mdadm.conf`, `mkfs.exfat` covered, and all fifteen binaries represented — 61 → 73**; **+2026-10-23 (ROADMAP 7.4): +11 over the `HostBins` table — the VPN/network/firewall/`mdadm`/diagnostics grants are rendered from it and pinned: no entry is a bare binary or reaches a shell, `pkill` is no longer granted anywhere, `wg` only names `wg0`, `openvpn` is confined to `/etc/openvpn` with a fixed tail, `ip` stays per object (never `netns`/`tunnel`/`rule`), `wpa_cli`/`ufw` per subcommand, `mdadm` covers both `--action` spellings, `smartctl` always ends on `/dev/*`, `tail` only the four whitelisted logs, and `dhclient`/`exportfs -ra` are covered, plus an unescaped-comma guard on every rendered grant (a comma is sudoers' command separator) — 73 → 85**, `storage/setup_state_repo.rs` (6 — `has_admin` true/false/admin-vs-user, `mark_complete` round-trip + upsert, `update_system_hash` inserts a missing row), `storage/setup_progress_repo.rs` (singleton `setup_progress` row — get/put round-trip, non-secret choices only), `middleware/setup_guard.rs` (8 — loopback, private, link-local, ULA, IPv4-mapped unwrapping, public rejected, HTTP/2 `:authority` fallback when `Host` header is absent, **+2026-10-18 (recs): `counts_against_setup_budget` — `/auth/setup/progress` is exempt from the 20/min bootstrap limiter, the three guess/enumerate routes stay metered**; **the same-origin and rate-limit checks are covered end-to-end in `setup_integration.rs`**)` | [x] `setup_integration.rs` (**60** — +1 HTTP/2 `:authority` same-origin check without `Host` header; +5 on 2026-10-13 (b): `/auth/setup/progress` GET/PUT round-trips the step + choices, both 403 once an admin exists and are behind the local-network guard. **+2026-10-18 (recs): `check-username` is now first-run-only (403 once an admin exists — it was a free username oracle post-setup) and `test_check_username_is_disabled_once_setup_complete` + `test_setup_progress_does_not_consume_the_bootstrap_budget` were added while `test_check_username_taken_after_setup` / `test_check_username_different_one_still_available` were deleted (they asserted the enumeration the change closes) — 60 → 60.** **+2026-10-17 (b): the `/auth/setup/storage-options` case was removed with the endpoint — 52 → 51.** **+2026-10-17 (c): +8 hardening cases — same-origin guard (foreign `Origin`, `Sec-Fetch-Site: cross-site`, `Origin: null`, same-origin allowed, header-less client allowed), per-peer rate limit (429), 11-char password, banned password — 51 → 60.** +13 on 2026-10-13: **DB-derived state** — an admin row with no marker files is not a first run, `/auth/setup` refuses it, `/auth/setup/verify-totp` 403s, and setup records the fingerprint in `setup_state`; **recovery** — `/auth/setup/rebaseline` requires a session, requires admin, and clears `needs_recovery`; **local-network guard** via `create_app` — a public peer is 403 on `/auth/setup`, `/auth/check-username` and `/auth/setup/verify-totp`, loopback and a LAN peer are allowed, `HOMEVAULT_SETUP_ALLOW_REMOTE` lifts it. +6 on 2026-09-22: TOTP `verify-totp` accepts/rejects + gated post-setup, setup rejects an invalid TOTP code without creating a user, setup with a valid TOTP = 2FA enabled + backup codes, secret alone ignored) | [x] `setup.spec.ts` (**96 `test(` / 100 runtime** — **+2026-10-18 (recs): re-run after the wizard fixes — 98 passed / 2 failed / 0 skipped (chromium, fresh first-run server), down from 23 failed. The two remaining failures are PRE-EXISTING and unrelated to the recommendations: 15.3 ("setup status check makes at most 3 API calls" — the load makes 4) and 16.2 ("rapid step navigation does not crash" — ends off `[data-step="1"]`); both failed identically on the pre-change binary (verified by stashing the work and rebuilding). Count unchanged** ; **+2026-10-18: full re-run on a fresh scratch server (isolated `DATA_DIRECTORY`, first-run state) — 78 passed / 0 failed (chromium); the remaining tests self-skip by design once 19.1 creates the admin and the server stops being first-run. Count unchanged** ; +19.0 on 2026-09-22: TOTP toggle at the password step → QR + secret + compatible-apps message, non-destructive; **+2026-10-17 (c): 5.7/5.8 count the 6-rule checklist, and 5.13-5.15 pin the 12-char floor, the banned-password rule and the repeated-run rule — 93 → 96**) ; **+2026-09-12: Appliance design migration — stepper + steps + admin creation + recovery + 2FA in Tailwind-on-tokens, selectors `data-testid="setup-*"`, zero scoped CSS** ; **+2026-09-22: TOTP moved INTO the password step (toggle → client `qrcode` QR → code verified via stateless `POST /auth/setup/verify-totp` then re-verified in `POST /auth/setup`), recovery code generated+displayed at step 4 after creation, post-creation 2FA screen removed — backup codes recap after recovery, section 19 rewritten (19.0 non-destructive + 19.1 full flow)** ; **+2026-10-13: the wizard is now rendered by `App.svelte` from `$appGate.route`, not by `Login.svelte`; the setup E2E still detects it via `[data-testid="setup-container"]`. Spec not re-run** ; **+2026-10-13 (b): the 8-step flow adds selectors `setup-stepper-label` (8 dots), `setup-language-{en,fr,de,es}`, `setup-storage-list`/`setup-storage-system`, `setup-raid-modes`/`setup-raid-{none,mirror,stripe}`, `setup-restore-choice`/`setup-restore-skip`/`setup-restore-enable`, and the `setup-resumed` banner. Spec not re-run** ; **+2026-10-17 (b): the storage/RAID/restore selectors were removed with the steps; the spec's step indices were renumbered to the 4-step flow (account 1, password 2, verify 3) via a `gotoStep` helper, `ensureWizard` rewinds to step 0 first (reboot-safe progress), the stepper counts dropped to 4, and new §18b/§18c run the wizard under a `fr-FR` and an `it-IT` Playwright locale to pin the OS-locale detection and its English fallback. Spec not re-run** ; **+2026-10-17: the wizard's own emerald family (`#10B981`/`#22C55E`/`#16A34A` + the `#10B981→#059669` create gradient — 2.28 to 3.77:1 wherever it carried ink) was replaced by the Appliance success ramp (`--hv-success` / `--hv-success-text`), the recovery box became white paper in a 2 px `--hv-warning` border with an amber `--hv-warning-light` code chip, and the wizard ink ramp dropped onto `--hv-shell-text-*`; guard = `login.spec.ts` 13.6 (count unchanged, spec not re-run)** |
| 3 | Desktop (desktop/shell) | [x] 2026-07-17 (+ ~15 follow-ups up to 09-04, incl. the glassmorphism fix of 09-05) | [x] 2026-09-09 — (+2026-10-31, QW-12: `src/global_search.rs` **17** — `validate_query` over the whole matrix (empty/whitespace, one character, lowercase+trim, accented input, `/` `\\` NUL refused, the 128-**character** cap counted in chars not bytes), `clamp_limit` bounds, `dedupe_roots` (same folder twice via `/tmp` and `/tmp/../tmp` → one walk, the first label wins; blank paths dropped; the 16-root cap), `merge_hits` (dedupe by path, exact→prefix→substring, directory before file at equal rank, deterministic path tie-break, truncation reported), `tag_hits`, `hit_is_confined` (a shared-prefix sibling and `/etc/passwd` both refused)) + `src/i18n.rs` **+1** (`file_search_keys_are_at_parity_in_every_locale`: `search.type_file` / `files_searching` / `files_partial` / `files_truncated` / `files_unavailable` present, non-empty and free of TOML `{name}` in all four locales, `%count%` kept) + `src/sse/progress.rs::tests` (20 — last event retained, `owner`, `list_activity`, cleanup; **+2026-10-12, 9 new**: 5 on `get_progress` now returning the job's real last event instead of the hardcoded `Running` / 0 % / `"Waiting..."` placeholder — seeded event before the first emit, mid-flight `%`/message/byte counters, completed at 100 %, failed, and unknown-or-removed id → `None`; 4 on the `JobType` serde shape — round-trip of every variant through JSON, byte-identical wire shape for the built-in kinds, `{"type":"custom","name":"explorer_copy"}` for a feature slug, and a whole `ProgressEvent` of a `Custom` job serializing non-empty) + `src/handlers/activity.rs::tests` (14 — `kind` mapping, aria2 name derivation, %, serialization) + `src/activity_poller.rs::tests` (**37** — 2026-10-24 (ROADMAP 28.3): the mdstat/zpool/btrfs/Syncthing parsers and the reconciliation, no hardware) and `src/sse/progress.rs` gains `emit_message_only` (+2, see the 28.3 section) for the **taskbar** | [x] `global_search_integration.rs` (**13**, 2026-10-31 QW-12 — two real roots on a temp dir: `401` with no token and with an unknown token; the cross-root criterion (a file that exists **only** in the second root is found, carries its real `root`, and the report lists both roots with `timed_out: []` / `truncated: false`); the same file name in two roots is **not** deduplicated away (one hit per root); every hit carries `root` + `root_kind`; `limit=1` answers one hit and `truncated: true`; `files_only=true` returns no directory; the four query refusals are `400` with a named rule (empty, one character, `../../etc/passwd`, 129 characters) and never a `200` with an empty list; a file outside the allowed roots (`/etc/hostname`) is never returned; an **admin** is confined to the same sidebar roots. Not covered here, deliberately: the per-root/total timeouts (they need a stalling mount ⇒ root) — the budgets are constants the fan-out reads, and the "did not answer" branch is asserted in E2E by mocking the endpoint) + `desktop_integration.rs` (3 — auth guards on `/api/server/restart` + `/shutdown`) + `activity_integration.rs` (**13** — +2026-10-24 (ROADMAP 28.2): a job that just failed is visible with `state: "failed"` and its reason in `detail`, a just-cancelled one carries `state: "cancelled"`, and every row carries its lifecycle slug; 11 before that — `GET /api/activity`: `require_auth` 400/401, pending/running/completed/failed lifecycle, indeterminate `%`, partitioning by `owner`, never spawns aria2) + `activity_poller_integration.rs` (**10** — 2026-10-24 (ROADMAP 28.3): a real `/proc/mdstat` fixture through `tick()`, read back over `GET /api/activity`; the one-row-per-operation rule, the sweep never touching a foreign job, a missing source as an empty feed) + `progress_integration.rs` (9 — `require_auth` 400/401 on `/api/progress/jobs` and `/{job_id}/status` and the SSE route, security-hardening headers, **+2026-10-12** `job_status_reports_the_real_last_event`: a real tracked job reads back its own `%`/message/state over HTTP, a completed one reads `completed`/100 %, an unknown id is a clean 400 — this test is what exposed the `JobType::Custom` serialization failure below, and **+2026-10-23 (ROADMAP 9.1)** `sse_stream_replays_a_terminal_event_that_already_happened`: a job failed *before* the client subscribed (`sudo` refused) must still deliver its terminal event and reason on the SSE stream, and the stream must end on its own) | [x] `desktop.spec.ts` (**91 runtime tests / 91 `test(`, 15 categories** — +2026-10-24 (ROADMAP 28.3): **13.11** a synthetic `raid_resync` row renders with a localised name and its `%`, then leaves the dock when the operation ends; +2026-10-24 (ROADMAP 28.1/28.2): **13.9** a failing `/api/activity` is polled with exponential backoff (≤3 requests in 7 s, against 4 for the old 2 s cadence) and **13.10** a failed job keeps a row with its outcome, no `role="progressbar"` and no dock badge; 88 runtime tests, 15 categories — **+2026-10-18: chrome redesign — both pills are capsules, all their icons are circles.** One change, two surfaces. (a) *Dock*: `.hv-dock` takes `--hv-radius-full` (the browser clamps it to height/2, so both ends are exact semicircles) and `.hv-dock-icon` becomes `border-radius: 50%`. **8.5 rewritten** — it used to protect “dock and topbar share one corner radius”, which the design now makes false by construction, so it asserts `computed radius >= height / 2` on **both** pills at 400/768/1440px (a test that kept the old equality would block the design instead of protecting it) — and **8.7 added**: every dock icon is a perfect circle (computed `50%`, square box, **one uniform size** per width; it waits for the staggered `hvDockItemIn` entrance to finish first, since a mid-animation tile measures at 0.9× and reports three different sizes). (b) *Topbar*: same capsule, and the three buttons drop the icon + label chip for a **44px perfect circle at every width**, name on `aria-label` + a dock-style CSS tooltip (`title` removed, the `desktop.power` key orphaned again and deleted from the 4 locales). (**+2026-10-31, QW-12: §6b "Cross-root file search", 6 tests** — a matching file is listed with **the folder it lives in** and a "File" type badge; picking it opens the Explorer on that folder and the intent path is on screen; a partial answer renders `search-files-note` naming **2** locations and still shows the no-results state; a failing provider keeps the local results and says it is unavailable; a one-character query issues **zero** network calls; the file path never appears in the URL. `desktop.spec.ts` goes 91 → **97 `test(`**. Written, **not run** in this session — the frontend was rebuilt but the suite was not executed, per instruction) Category 15 was renamed “Topbar circular icon buttons” and 15.1/15.2/15.4/15.5/15.6/15.7 rewritten on the new contract: circle radius/size/uniformity at 400/900/768/1440px, no label node, badge anchored to the circular button's corner **and confined to its top-right quadrant** (a 26px glyph in a 44px circle leaves no clear band — their boxes overlap by 4px, as in the mockup), tooltip `content` === `aria-label` read through `expect.poll` (the 120ms `--hv-chrome-tooltip-delay` makes a synchronous read see `opacity: 0`), `title` asserted `null`, and the pill no longer widens with the language. Count unchanged: `grep -cE '^\s*test\('` = 83. (c) **The suite acknowledges the release notes in `beforeAll`** (`POST /api/whats-new/seen` with `X-Session-Token` + `X-CSRF-Token`): on a freshly seeded server the What's New modal opens over the desktop and its bits-ui overlay swallows **every pointer event** — that, not the design, is what made 14.7/14.8 and 16.2/16.4 fail on a fresh scratch server, and the four failures moved between runs. **Verified live** on a scratch server (`PORT=8101`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup`, Firefox): the full file gives **74 passed / 6 skipped / 3 failed**, and the 3 are pre-existing and unrelated to the chrome — 6.4 (`Escape closes search`, green when rerun alone: a load flake), 16.2 (severity chips: `aria-pressed` never flips after focus + Enter) and 16.4 (the 426 reload banner never appears) — both of the latter failing on this server **before** any chrome change. 8.5, 8.7 and 15.1-15.7 are green on **Firefox and Chromium**. **+2026-10-17 (b): 7.4 added — the power dialog and the topbar popovers share the `--hv-radius-4xl` 24px chrome corner (the power dialog moved off its off-scale 28px); `grep -cE '^\s*test\('` and `--list` both give 82. Spec not re-run**; recounted on 2026-09-24: +11.3 "a crashing window body shows the alert fallback and Reload recovers it" (`svelte:boundary` guard, W10 — written with the rest of the pass, **not executed**; `grep -cE '^\s*test\('` **and** `npx playwright test --list` give 74); 73 on 2026-09-17: +14.11 "every app header bar carries the icon + title only", which iterates the 24 app ids of the registry and requires icon + title only in the bar; 72 on 2026-09-16 via `grep -cE '^\s*test\('` **and** `npx playwright test --list`, both give 72 (the file's method); +cat. 13 on 2026-09-09: taskbar (dock button + badge, panel width = dock, name/%/track row, row click = nothing, close via Escape/outside/re-click, responsive, localStorage/URL safety, poll `/api/activity` never `/downloader/tasks`); **never confirms** a restart/shutdown/logout; +2026-09-12: chrome geometry tests — 8.5 dock/topbar pills same radius at each width, 8.6 maximized window does not overlap the dock on tablet; selector fix 5.3 (`.jr-page` renamed → dialog `aria-label="System Journal"`); **+2026-09-14: dock/topbar fidelity to the approved gateway mockup — 8.3 rewritten** (each pill is worth **its own** token, `--hv-chrome-topbar-h` / `--hv-chrome-dock-h`, and the dock is strictly taller: the old "same height" invariant is replaced by its opposite, otherwise the test would no longer protect anything) **+ cat. 14 "Chrome geometry & material"** (14.1 the JS mirror `snapUtils.ts` exactly copies the CSS keep-outs; 14.2 the `--hv-app-<id>-c` badge of each tile is indeed the dark stop of its gradient; 14.3 the two pills are opaque — `backdrop-filter:none` + `rgb()` without alpha; 14.4 the hover tooltip, until now uncovered: content = `aria-label`, background `--hv-tooltip-bg`, radius and font size); 8.4 and 8.5 stay green without modification — the avatar shrinks with its frame, the pill radius has not moved; **+2026-09-14: topbar buttons as icon + label chips (approved mockup, node `1:24`) — cat. 15 "Topbar icon + label chips"**: 15.1 at 1440px each button shows icon **and** label (width > 44, height = 44, label to the right of the icon and centered to ±2px); 15.2 below 901px the label is `toHaveCount(1)` **then** hidden (present-but-hidden, a single markup branch) and the button becomes a 44×44 square again; 15.3 real DOM order `[network, widgets, notifications, power]` **and** Power stays the last `.hv-topbar-btn` (the positional hypothesis of 7.1-7.3 is kept as an assertion, even though those three tests now use `[data-testid="topbar-power"]`); 15.4 the notification badge is **16px** (the probe is a clone of a template element — Svelte CSS is scoped by hash, a `createElement` would measure an unstyled span), `.hv-dock-badge` keeps its **26px**, and the badge lives in the band **above** the label line (`badge.bottom <= label.top`) so a "99+" widens into empty space instead of covering the text; 15.5 translated accessible name (never a raw key) and, at 1440px, containing the visible label — WCAG 2.5.3, which validates the `power_options`/`power` pair; 15.6 at **901px** (the blind spot of 8.2, which only samples 360/768/1920) the right-anchored pill stays at `left >= 0` and without horizontal scroll; 15.7 the 4 locales render 4 non-empty and non-key labels, then the **German** pill (the worst width case) still fits at 901px; 8.3 / 14.1 / 14.3 / 14.4 stay green **without modification** — proof that the delta is purely horizontal; **+2026-09-14 (same day): app window chrome = "AppWindow" mockup (Figma `4pwNRZU5wQjSWQBODFViKz` node `3:76`) — cat. 14 completed by 14.5 opaque surface `#FFFFFF` + slate-300 hairline + two-layer shadow + `backdrop-filter:none`, 14.6 44px bar / 24px radius 8 tile / 14px 600 `#314158` title on 20px, 14.7 28×28 radius 8 gap 6 trio at the stops `#FFB900`/`#2B7FFF`/`#FF6467` **in the bar** (inter-button gap and 12px inset measured, not read in the tokens) and 14.8 the `--hv-winhdr-btn-clear` reserve really protects the title — **measured** at 1440/768/400px, because the trio is absolutely positioned: the right padding of the bar is the only thing preventing it from going underneath. **14.9 (a single header per window) was written after finding a real bug**: `quotas`, added on 2026-09-14, was missing from the `OWN_HEADER_APPS` `Set` of `WindowFrame.svelte` → the Quotas window stacked two bars (its own + the empty WindowFrame fallback); fixed by adding `'quotas'` to the `Set`. Categories 3 and 15 stay green **as they are**: no `data-testid` moved, only the size, color and position of the trio changed; **+2026-09-14 (same day): standardization of headers + dock tile gap — 14.6 hardened** (the bar is now title-only: the icon tile is worth `--hv-winhdr-icon-size` = `--hv-winctrl-size`, i.e. the same 28px as the trio, and the measured height is **exactly 44px** — the test goes from a floor `>= 44` to an equality, which turns any element that would come back into the bar into a failure); **14.7 completed** (the median lines of the tile, the trio and the title are compared to that of the bar, not just their sizes); **14.10 new** (the `gap` of `.hv-dock-container` goes to `--hv-chrome-dock-gap`, 10px: on hover a tile grows by `48 × (1.18 − 1) / 2 ≈ 4.3px` on each side, so the old `gap` of 4px made two neighbors **actually** overlap; the test hovers a middle tile and requires the measured clearance at the two neighbors to exceed the growth actually produced — it reads no token); 14.1 to 14.5 and 14.8 to 14.9 unchanged. **Specs modified but NOT re-run** — the session instruction was explicitly to neither compile nor test) ; **+2026-09-16: redesign of the app launcher on the Figma mockup `4pwNRZU5wQjSWQBODFViKz` node `23:6646` ("Desktop launcher") — `HvLaunchpad.svelte` goes from an opaque centered modal (1000×720, vertical category rail, wide cards with description) to a **darkened full-screen veil** (`.lp-overlay`, `rgba(15,23,42,0.72)` + `backdrop-filter`, content column aligned at the top (704px at the redesign; **828px** since the "6 columns" request of the same day)), a **row of category chips** (`aria-pressed` in a `role="group"`, not a `tablist`: they filter the same grid in place) and a **compact tile grid** (`repeat(auto-fill, minmax(124px, 1fr))`, 56px icon above the label on 2 lines max). New page-scoped palette `--hv-launcher-*`; **two values of the mockup were corrected at the WCAG contrast threshold** — the veil `0.55` → **`0.72`** (the white labels fell to 3.39:1 on a light wallpaper) and the inactive chip text 85% alpha → **opaque white** (4.28:1); the placeholder `#90A1B9` (2.6:1 on the white bar) goes to `#64748B` and the secondary ink 72% → 88% alpha (3.8 → 4.7:1). The tiles keep the **per-app gradient** (`--hv-app-<id>-grad`) and not the white square of the mockup — the "one color per app" rule is binding; **no invented counter badge** (the mockup shows some, the data does not exist): the only corner badge is green and **is rendered only if the container is running** (a gray "stopped" dot is undetectable on the tile background, ~1:1; the stopped state is already carried by `aria-describedby`). The 8 `desktop.app_<id>_info` warnings (Rescue, RAID, Storage, Network, VPN, Backup, Users, Docker) survive as an **accessible description** (`.lp-sr` + `aria-describedby`) instead of being orphaned. The scrim becomes a real `<button>` sibling of the dialog (no more `role="button"` nesting `<button>`s). **E2E contract unchanged**: `.lp-overlay`, `.lp-panel`, `.lp-card[title]`, `.lp-search-field`, the external Docker menu (`.lp-ext-*`), `.lp-empty`/`.lp-reset`, `.lp-count` keep their selectors — 5.1 to 5.4 pass as is; **category 5 extended from 4 to 12 tests**: 5.5 the chips filter the grid in place + exactly one `aria-pressed="true"`, "All apps" restores; 5.6 search with no result → `.lp-empty` → `.lp-reset` restores the grid and clears the field; 5.7 the launcher is a `fixed` **dark translucent** veil and the content column paints no surface; 5.8 **measured contrast** — the composite tile label tile→veil→white paper (the worst case of a wallpaper) and requires ≥ 4.5:1, opaque ink; 5.9 the chip paints < 44px but carries a 44px `::after`, the search field is 44px in the 56px bar of the mockup, the tile ≥ 44px; 5.10 the focus trap holds 30 Tab and Escape **returns focus to the dock tile** (2.1.1 / 2.4.3); **5.11 (phone)** 3-column grid, `nowrap`/`overflow-x:auto` chips with `flex: none` (**the desktop growth is cancelled** — a stretched chip would fill the row and there would be nothing left to scroll) and a row **actually overflowing**, the panel **scrolls internally** (neither the page nor the horizontal axis moves) and the last tile remains reachable; **5.12 (6 columns — request of 2026-09-16)** the grid makes exactly 6 tracks of ≥ 124px at 1440px, **and** the search bar, the chip row and the grid share the same width **and** the same `x` (1px sub-pixel tolerance: it is a layout misalignment that must fail, not a rounding), **and** the content of the chip row reaches the right edge of the grid (**request "not enough categories → enlarge the module" of the same day**: the box of the row already measured 828px, but a chip is only as wide as its label, so with few categories the chips stopped at two thirds and the module *appeared* narrower than the tiles; `flex: 1 1 auto` shares the free space of each line among its chips, and the assertion is made **per rendered line** so that it also holds when the row wraps — a complete catalog goes up to 13 chips; `grow` rather than `space-between`: both spread the last line, but `grow` spreads it into slightly wider chips whereas `space-between` opens a gap between two distant chips) — `--hv-launcher-content-w` goes from 704 to **828px** (6 × 124 + 5 × 12 = 804 minimum; 828 lands on round 128px columns). The three modules being children of the same column, they widen together — it is the test that prevents it from drifting. The scrollable container moved from `.lp-content` to **`.lp-panel`** (with `scrollbar-gutter: stable` and a `sticky` search bar) precisely for that: a scrollbar that takes width (Chrome Windows/Linux, GTK without overlay) took it away from the grid alone, the search bar and the chips being outside that container; 5.12 therefore measures the three widths **a second time at 1440×520**, with overflowing tiles — the only regime where they can diverge (the E2E engine's scrollbar is an overlay, 0px, and hides the drift). **Two real defects found by these tests, not by re-reading**: (1) 5.5 and 5.6 failed because the `<svelte:window onclick>` of `Desktop.svelte` closes the launcher on every click — `.lp-panel` took back its stopPropagation `onclick` (and above all no `onkeydown`, which swallowed Escape: it is a bug already seen); (2) the measurement of the `backdrop-filter` of 5.7 showed that declaring the pair `backdrop-filter` + `-webkit-backdrop-filter` makes the minifier keep the **only** prefixed form, which Firefox ignores (`CSS.supports('-webkit-backdrop-filter', …)` = false) — the `blur(2px)` of the mockup was therefore worth `none`. Written as a standard property alone, the minifier itself emits both forms. **72 tests / 15 categories** ; **+2026-10-18 (d): the app-header icon tile is a perfect circle** — `--hv-winhdr-icon-radius` goes from `8px` to `50%`, the same shape language as `.hv-dock-icon` and `.hv-topbar-btn`, on both `WindowHeader` (apps) and `WidgetHeader` (the 11 desktop monitors); the min/max/close trio deliberately stays an 8px square (`--hv-winctrl-radius`), so only the identity glyph is round. **14.6 updated** (not added): its `parseFloat(radius) === 8` assertion becomes `borderTopLeftRadius === '50%'` — a px stop would square the tile back the moment `--hv-winhdr-icon-size` moves, and `parseFloat('50%')` is `50`, so the old form could not express the new contract. `widgets.spec.ts` **2.2** gains the matching `tileRadius === '50%'` assertion (it already measured the tile's 28×28 box and centring). Counts unchanged (`grep -cE '^\s*test\('` = 72 desktop). **+2026-10-18 (e): the window control trio is circular too** — `--hv-winctrl-radius` goes from `8px` to `50%`, so the min/max/close buttons join the dock tiles / topbar buttons / header icon tile in one shape language; the 34px transparent `::after` hit pad takes a literal `50%` instead of `calc(var(--hv-winctrl-radius) + 3px)`, which would have been a meaningless `calc(50% + 3px)` once the token moved. `WindowHeader`'s `.hv-winhdr-action-btn` stops borrowing that token and takes `--hv-radius-control` (8px): it is a *labelled* button, and `50%` would have turned it into a pill (no app fills that slot today, but the prop stays). **14.7 updated** (not added): `parseFloat(radius) === 8` → `borderTopLeftRadius === '50%'` for each of the three buttons. **Specs modified but NOT re-run** ; **+2026-10-18 (f): dock proportions taken from the approved reference dock** — the dock tile grows from 48px to **64px** (`--hv-dock-item-size`, which existed as a dead 62px token and is now the tile's single source), the bar's padding tightens from 12px to **10px** (`(84 − 64) / 2`) and the tile glyph goes 24px → 32px (0.5 × tile, unchanged ratio). The point is the *ratio*, not the pixels: the reference's circular tiles measure 158px in a 204px bar → **0.775 × the bar height**, with 0.146 × tile of padding and a 0.152 × tile pitch, and 64/84 = 0.762 + 10px + 10px reproduces it exactly inside the existing `--hv-chrome-dock-h` (84px), so the keep-out, the JS mirror and every dock-anchored panel are untouched. The hover clearance is now `10 − 64 × 0.18 / 2 = 4.2px` per side (was ~6px at 48px) — still positive, which is all E2E `14.10` requires; its comment was updated to the new arithmetic, and `15.4`'s comment now says the 26px dock badge sits on a 64px tile. **No assertion changed** (8.5/8.7 pin shapes and uniformity, not absolute sizes). **Verified live** on a scratch server (`PORT=8099`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup`, Chromium): dock 84px tall, tiles 64×64 (ratio 0.762), padding 10px, measured gaps 10px, hovered tile 76px wide with **4.24px** of clearance to each neighbour; 400/768/1440px render without horizontal overflow, and the phone regime keeps its own 56px bar / 44px tiles (ratio 0.79). `8.5`, `8.7`, `14.1`, `14.10` green (Firefox). Counts unchanged ; **+2026-10-18 (g): the widget headers were brought onto the app header's icon contract** — the 11 monitor icon snippets in `Desktop.svelte` passed `size={16}`, which the CSS clamp (`.hv-widget-tile :global(svg) { width/height: 16px }`) already owns and therefore overrides: dead noise, exactly what the 24 app header snippets had removed on 2026-09-14. They also carried `stroke={2}` while the app-header family uses 1.7–1.8, so the two bars' glyphs read at different weights side by side on the same desktop. Both fixed in one pass (`size` dropped, `stroke` → 1.8), and the rule is now documented on `WidgetHeader.svelte` itself. **Audit of both header families, measured live** (Chromium, scratch server `PORT=8099`, `e2eadmin`-style seeded admin): all 11 widget bars are 44px tall with a 28×28 tile at computed `50%`, a 16×16 glyph (SVG attribute 24, clamped by CSS), tile centred to ≤0.5px, 12px left gap, title 14px/600 `#314158`, bar starting 1px inside the panel's hairline (so the border is not covered); the app window header is 44px with a 28×28 `50%` tile, a 28×28 `50%` trio at 12px inset / 8px top, every element on one centre line, and no title/trio overlap at 1440/768/400px (maximized included). No assertion changed — `widgets.spec.ts` `2.2` and `desktop.spec.ts` `14.6`/`14.7` already pinned those numbers, and the glyph contract has no E2E probe (it is a CSS-ownership rule, verified by measurement here). **Verified live, specs re-run** ; **+2026-10-18 (h): one surface corner for every container** — the radius scale carried three container stops (16px windows, 24px chrome panels, 12px cards), so a window next to a popover read as two different products. `--hv-radius-2xl` is now **12px** and `3xl`/`4xl` alias it; `--hv-window-radius` and `--hv-radius-card` resolve to it too, leaving **one** surface value plus the control family (4/6/8/10px) and the capsule. **Three assertions updated, none added**: `desktop.spec.ts` `14.5` (window radius 16 → 12), `7.4` (popover + power dialog 24 → 12, title renamed), `login.spec.ts` `13.7` (setup card 16 → 12, title renamed). `widgets.spec.ts` `2.2` already expected 12 and needed no change — which is the point: the widget panels were the only containers already on the right stop. **Verified live** (Chromium, scratch server `:8099`): computed `12px` on the app window, all 11 monitor panels, the topbar account popover, the power dialog, a Users/Groups modal, a launcher tile, and the token set reads `xl/2xl/3xl/4xl/card/window = 12px` with `--hv-radius-control = 8px`; capsules (launcher search bar, category chips) and circles (dock/topbar/header tiles, control trio) unchanged. `7.4`, `14.5`, `2.2`, `13.7` green (Firefox). Counts unchanged ; **+2026-10-18 (i): one control corner too — buttons, inputs and selects take 6px** — the control family carried three stops (6px `default`, 8px `md`/`control`, 10px `lg`), so the shared `Button` (vendored shadcn `rounded-md`), the settings cards' `lg` buttons and the `PathField` inputs did not share a corner, and a button next to the input it submits did not line up. `--hv-radius-md`/`--hv-radius-lg`/`--hv-radius-control` all resolve to **6px** now; the vendored `rounded-[min(var(--radius-md),10px)]` size variants collapse to 6px for free. `--hv-radius-sm` (4px) stays for inline badges, which are not controls. **One assertion updated**: `login.spec.ts` `13.7` (field 8 → 6, title renamed). The stale `var(--hv-radius-md, 8px)` fallbacks in `PathField`/`PathPickerField` were dropped — the token always resolves and a wrong fallback is what the audit tasklist flagged. **Verified live** (Chromium, `:8099`): token set reads `sm=4 default=6 md=6 lg=6 control=6 2xl=12`; in Settings the primary button (123×44) and every input/select compute **6px**, the sidebar nav rows keep their 12px container corner, and the window's control trio stays `50%`. `13.7` green (Firefox). Counts unchanged ; **+2026-10-18 (j): every button on every screen takes the one control corner, and 14.12 enforces it** — the 6px token from (i) only fixed the *shared* control classes: ~25 hand-written buttons still carried a Tailwind `rounded-xl` / `rounded-2xl` (a surface corner on a control), so Storage's disk cards, the RAID wizard's option cards, Settings' category rows, the chat send/history tiles, the VPN row actions and the Users role cards read at 12px while the buttons beside them were 6px. One pass fixed them (`rounded-xl`/`rounded-2xl` → `rounded-lg` **inside `<button>` tags only**, via a brace/quote-aware tag scanner so `onclick={() => …}` could not truncate the match; plus the class *constants* that feed buttons: RAID's `typeCard`/`diskRow`/`fsCard`, Users' `railRow`/`iconBtn`, the launcher's `.lp-card`/`.lp-reset`, the power dialog's `.hv-power-option`, the account menu's `.hv-user-menu-item`, the network popover's `.hv-net-copy`, and `.hv-ic` / `.hv-winhdr-tab` which had no radius at all). **New test `14.12` "every button on every screen carries the one control corner"** walks the 23-app registry like `14.11`, collects the computed corner of every `button, [role="button"]` per app, and accepts only `6px` / `50%` (circular shape language) / `9999px` (capsule chips) / `0px` (hairline-separated table rows and segmented-control children) — every other value fails with the offending classes. It also asserts `--hv-radius-control` really is `6px`, so the test cannot pass on a coincidence. **Audit that motivated it, measured live** (Chromium, `:8099`): before, 250 buttons at 6px but 23 at 12px and 27 at 0px; after, **256 at 6px, 0 stray**, with the remaining `0px` being Explorer's 26 file-list rows and the Journal's 200 log rows (both `role="button"` with `border-b` hairlines) and 2 Rescue segmented-control children. Modals audited separately (175 buttons, all 6px) and the desktop chrome too (account popover, network popover, power dialog, launcher: 6px; dock/topbar buttons 50%; capsule chips 9999px). **14.12 green (Firefox, 10.5s)**; `14.6`/`14.7`/`14.11`, `5.9`/`5.12`, the Docker suite (56 passed) and Storage/Chat stay green. `desktop.spec.ts` count goes 74 → **75** ; **+2026-10-18 (k): the frame footer's glyphs take the header's step, and 14.13 pins it** — the shared 36px strip shipped with 16px help / resize-grip glyphs against the trio's 14px ones, and because the footer assets are full-bleed circles (18.5/24 of their box) their ink measured 13.75px against the trio's 8–12px: the footer read a size larger than the bar above it. Both now clamp to one new token, `--hv-winctrl-glyph` (14px), the footer SVGs drop their `width`/`height` attributes (CSS owns the box, exactly like the header's app icon tile), and **14.13 added** measures the two rows: trio glyphs 14×14, both footer glyphs the same box on 28px buttons, only the grip rotated 45°. **Verified live** (Chromium, scratch server `:8099`, isolated `DATA_DIRECTORY`): before, help / grip 16px (13.75px of ink, rotated grip bbox 22.6px); after, 14px (12px of ink, grip bbox 19.8px) — the maximize glyph is 12×12, so the two rows now match. `14.13` green; `14.5`–`14.8` fail on this working tree for **unrelated, pre-existing** reasons (the in-flight shadow/radius retune, plus a window entrance-scale flake that measures the tile at 27.86–27.96px instead of 28 and the trio inset at 108 instead of 109, with values that move between runs). `grep -cE '^\s*test\('` = **86** ; **+2026-10-19: the launcher's search bar loses its blue ring, and 5.13 pins the replacement** — the brand focus ring sat on screen permanently (the dialog's focus trap lands on the query field when the launcher opens) and was painted twice, since `base.css`'s global `input:focus` (0,1,1) outranks the plain `.lp-search-field` class and stamped a second ring inside the capsule. The bar now takes `inset 0 0 0 2px rgba(15,23,42,0.72)` and the field paints none. **5.13 added**: it reads the computed shadows of the bar and the field, requires no `rgba(30, 98, 174, …)` anywhere, no shadow on the field, an `inset` rim whose composite over the white capsule clears **3:1** (measured 7.1:1), and the rim gone once Tab moves focus to a chip. **Verified live** (Chromium, scratch server `:8099`, isolated `DATA_DIRECTORY`): before, the bar computed `rgba(30, 98, 174, 0.9) 0 0 0 2px, rgba(30, 98, 174, 0.35) 0 0 0 4px` **and** the field carried the same pair; after, the bar carries only the slate inset rim and the field `box-shadow: none`, with a pixel scan of the capsule edge showing `rgb(82,88,101)` over white and no blue ring (the only bluish pixels left in the crop are the wallpaper showing through the scrim). `5.13` green **and proven to bite** — re-run against the pre-change component it fails on the exact old `30, 98, 174` values. The whole launcher category (5.1–5.13) and 15.1–15.7 stay green. ; **+2026-10-22: the launcher grid reads A→Z and the veil hides the desktop** — user feedback on both counts. (a) *Order*: the tiles were sorted by a hand-kept `CAT_ORDER`, then alphabetically inside each category; the first tile was therefore "System" and a user scanning for an app had no alphabetical entry point. `CAT_ORDER`/`catRank()` are gone — the grid is one A→Z list (`Intl.Collator` on `$languageStore`, `sensitivity: 'base'` so case/accents fold and `numeric` so "App 2" precedes "App 10"), the chip filter applied on top, and the category chips are sorted by their **localised** label so the row reads in the same order as the tiles under it. **5.14 added**: it compares the rendered tile labels and the chips (minus the pinned "All apps") against the browser's own `Intl.Collator` on `document.documentElement.lang`, then re-checks the tiles after clicking a chip — the filter must not re-group the survivors. (b) *Veil*: `--hv-launcher-scrim` goes `rgba(15,23,42,0.72)` → **`0.86`** and `--hv-launcher-blur` `2px` → **`6px`**, because the desktop's icons and wallpaper stayed legible behind the launcher and it read as a translucent panel over the desktop instead of an app screen. Contrast only improves (the worst-case tile label tile→veil→white paper goes 5.5:1 → **8.7:1**); `5.7` (translucent dark veil, non-zero blur) and `5.8` (≥ 4.5:1) stay green unmodified. **Verified live** (Chromium, `:8080`, dev server rebuilt so the embedded assets matched): `grep -cE '^\s*test\('` and `npx playwright test -g "5\."` both give **21** for the launcher category, all green, and the French UI renders "Assistant IA, Docker, Edit Widgets, Explorateur, …" with the chips "Productivité, Réseau, Stockage, Supervision, Système". `desktop.spec.ts` count 86 → **87** ; **+2026-10-19 (b): the desktop power menu wears the dock's circular tiles, and 7.5 pins it** — user request: the three actions behind the topbar Power button read as three rounded cards while the dock next to them is a row of circles. `.hv-power-option` is now a transparent button (one control corner, shared focus ring) wrapping a new `.hv-power-tile` — a `--hv-dock-item-size` circle (56px desktop / 48px phone, still above the 44px floor), glyph at half the tile like `.hv-dock-icon`, one gradient per action (red `--hv-error`, blue `--hv-primary`, slate `--hv-neutral-700`→`900`, all ≥ 4.9:1 for the white glyph) and the dock's hover lift + `brightness(1.05)` (flattened under `@media (hover: none)`). The label moves **under** the tile; the 480px block that used to stack the sheet into one column keeps its 3 columns (a vertical column of circles stops reading as an action sheet) and only steps the tiles down. **`7.5` added** — it reads `--hv-dock-item-size` and `--hv-radius-control` off `:root` and requires each tile to be `50%`, that diameter, square, on a `linear-gradient`, the three gradients to be **distinct**, the label below the tile, and the button ≥ 44px on the control corner. It reads the tiles' **computed** width/height, not `getBoundingClientRect()`: the dialog's 200ms `hvPowerAppear` entrance scales the sheet to 0.95×, which is exactly the 53.2px the first version measured. `grep -cE '^\s*test\('` = **88**. **Verified live** (Chromium/Firefox via the Vite dev server on `:5173`, proxied to the seeded `:8080` backend): 7.1–7.5 green; at 1280px the sheet is 380×115 with three 56×56 tiles and 110×81 buttons, at 400px 360×106 with 48×48 tiles and 106×72 buttons, no horizontal overflow in either regime. ; **+2026-10-19 (c): the three topbar circles and their popovers moved onto the theme ladder** — user request after the account-menu pass. The buttons themselves were already one shape (56/44px circle, glyph at half, uniform per width — 15.1/15.2 pin it) but painted a bespoke inset focus `outline` instead of `--hv-focus-ring`; the **network** popover used one-off literals (20px padding, 12px gaps, **84px** rows, 52px icon tiles, a 48px copy button) and a **22px** IP value (`--hv-text-2xl`, the page-title step) for a row value; the **notification** panel titled itself at 17px (`--hv-font-size-lg`, the *section* step) with 12px form text, and — the real bug — its root had **no** `stopPropagation`, so a click on a severity chip, the search field or mark-as-read bubbled to the shell's global `svelte:window onclick` and closed the panel it lived in (**16.2 could never pass**; the other two popovers already carried that guard). Now: content-driven ~60px rows with 40px tiles and a 44px copy button, 15px row values (`--hv-text-md`, mono) and 15px panel title, 13px form text, the shared focus ring everywhere. The power confirmation sheet got the same pass — 18px glyphs (was 19/20), `--hv-radius-control` + 44px buttons, and a focus ring on its close button and footer actions, which had **none** (WCAG 2.4.7). Also fixed: **16.4**'s route glob was `**/api/health/**` while the desktop calls `/api/health`, so the mocked 426 never fired and the banner assertion could not pass (now `**/api/health*`). **Verified live** (Firefox, `:8080`, binary rebuilt so the embedded assets matched): network 440×267 with 67px rows / 40px tiles / 44px copy / 15px IP, notification header 15px + 13px search, the three buttons 56×56 with 28px glyphs and the shared ring under keyboard focus; `4.1`–`4.4`, `16.1`–`16.4`, `15.1`–`15.7` green (**27 passed**), the 4 reds are the pre-existing `14.5`/`14.7`/`14.8`/`14.13` window-frame work (a 27.98px sub-pixel flake), untouched by this pass. `grep -cE '^\s*test\('` unchanged (assertions fixed, none added) |
| 4 | Account | [x] 2026-07-13 (+09-03, 09-04) | [ ] shared with `auth/` only, nothing specific to Account (the passkey verifier is `src/passkey.rs`, **42 unit**, listed on its own) | [x] `passkey_integration.rs` (**16**, 2026-10-31 P1.3 — a **software authenticator** built in the test (a real P-256 pair, a byte-built attestation object, real signatures) registers through `/api/passkeys/register/{begin,finish}` and then signs in through `/api/passkeys/login/{begin,finish}`, producing a session and persisting the counter; the replay of a consumed challenge, a mismatched credential id, a foreign account's rename/revoke (`404`, never `403`), a bad signature from another key, an unknown credential (one sentence, no enumeration), a locked account (a passkey is not a way around an admin's lock), an expired challenge, the indistinguishable `login/begin` shape for a known vs unknown account, the anonymous discoverable begin, the trimmed/overlong label, and that registering a passkey leaves `user_2fa` untouched) + `account_integration.rs` (**31** — **+2026-10-24 (ROADMAP X.7): `test_security_endpoints_reject_an_expired_session` — an expired session is `401` on `/security/sessions`, `/revoke` and `/revoke-all`, and a refused revoke deletes nothing** ; +5 on 2026-10-22 (ROADMAP 14): `test_my_sessions_never_returns_a_live_session_token` (the payload carries a 12-hex opaque `id` and neither live token), `test_revoke_sessions_is_scoped_to_the_caller` (a foreign token/id is a 404 and the other account stays signed in), `test_revoke_sessions_protects_the_current_session` (current id refused; a mixed list revokes only the others), `test_revoke_sessions_rejects_an_empty_id_list`, `test_revoke_all_sessions_stays_scoped_to_the_caller`) | [x] `account.spec.ts` (75 + **§18 Passkeys, 8 tests (2026-10-31 P1.3)** — the card renders with its add control; an empty list says so; a registered passkey shows its label and the origin it is bound to; revoking asks first and **never fires when cancelled**; a failing list surfaces `passkeys-err` and *not* the empty state; no credential id reaches the URL or localStorage; readable at 360/768/1920; a browser without WebAuthn gets `passkeys-unsupported` instead of a dead button. Written against a **mocked endpoint** — the ceremonies need a real authenticator, which a headless browser has none of, and the crypto is covered by `passkey_integration.rs`. **Not run** in this session — the number `playwright test --list` reports; `grep -cE '^\s*test\('` says 73 because the `10.x` responsive loop declares one `test()` for three viewports, so the grep method undercounts by 2. The 2026-09-14 entry's "72" was that grep count, superseded by the measured 75 on 2026-10-22) ; **+2026-09-12: Appliance design migration — `PageLayout` + `SectionCard`, 2-col grid via Tailwind container queries (`@container`/`@max-*`), selectors `data-testid="account-*"`, zero scoped CSS** ; **+2026-09-14: standardization of headers — 1.2 now verifies `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; test 1.3 "the admin shows the administrator subtitle" is DELETED (the subtitle no longer exists) and the following tests of category 1 renumbered 1.3/1.4 so as not to leave a hole — that is the cause of the −1; 17.1 verifies the same invariant. Specs modified but NOT re-run** ; **+2026-09-22: self-service session management — two `SectionCard` "Active sessions" (list IP/created/expires, "This device" badge, per-session revocation via `POST /api/security/sessions/revoke`, "Log out all others" via `/revoke-all` with `ConfirmDialog`, refresh button) and "Recent logins" (`GET /api/security/login-history/{me}`: IP, success/failure, timestamp, user-agent); 15 i18n keys ×4 locales under `[account]`; existing Account specs not re-run** ; **+2026-10-22 (ROADMAP 14): 14.6 — the sessions list carries 12-hex opaque ids and neither live token in the payload or the DOM; revoking by id kills only that session; a foreign token submitted as an id is a 404 → 75** |
| 5 | Users | [x] 2026-07-13 (+ 3 bugs found by E2E on 09-04, +1 bug found/fixed by static review/tests on 09-05; +2026-09-08: home directory size slider = `setquota -u` disk quota; +2026-09-08: single-view design redesign in the approved template style — thumbnail grid → settings-list sheet → creation form, markup/CSS only; +2026-09-08: folder picker for the home directory (server validation `validate_custom_home_path`), size slider for all roles, explicit error state when the group list fails, `homevault_admin` displayed as a read-only chip derived from the role) | [x] `groups.rs` (**9** — +4 on 2026-10-21: `normalize_member_list` trim/dedupe, blank-name rejection, the 500-member cap, empty list) + `host_accounts.rs` (**14**, 2026-10-21 — username gate (option-like, shell/path metacharacters, non-ASCII, 32-byte cap), `is_system_uid` boundaries (0/999/1000/65533/65534), `HOST_ADMIN_GROUPS` covers sudo+wheel+homevault_admin, `chpasswd` line + newline-injection rejection, home path, no-op paths for a missing account) + `handlers/users.rs` (13, `is_valid_email`) + `quota.rs` (**20** — **+2026-10-22 (ROADMAP 3.1–3.3), 2 → 21**: the backend matrix for every filesystem (VFS `usrquota`, XFS, ZFS, Btrfs/other → unsupported), `mount_has_usrquota`/`quota_mount_options`, `findmnt -J` parsing (JSON array, missing options, failure), `repquota -p` parsing (grace column, `-`/0 sentinels, hard+soft sum, unknown user), ZFS `userquota@` value and `zfs userspace` parsing, `zfs_quota_value`, `setquota_args` (hard == soft, mount last, no shell metacharacter reachable), `privileged_argv` (`sudo -n`, tool first, no shell), `bytes_to_quota_kb`/`format_size_kb` rounding and clamping, and `reason_code` snake_case wire serialisation) + `handlers/user_homes.rs` (8 — `validate_custom_home_path`: rejects relative/traversal/control characters/root/system dirs/missing parent, `containing_partition` longest-prefix) | [x] `users_integration.rs` (**79** — **+2026-10-21 (host lifecycle, no root needed, 71 → 79)**: `is_host_account` on list + detail, a stale PAM-mirror row is deletable while a live one is not, the home-config row goes with the account even when files are kept, demotion revokes the demoted account's sessions, the 50-group cap, an unknown group in `PUT .../groups` → 400, a blank group name rejected; 71 before — +2026-10-12: `test_user_delete_revokes_its_direct_share_permissions_only`, the regression guard for the direct grants left behind by a deleted account; +2 before: `PUT .../full` with `home_path` rejects a path under a system folder + relative/traversal/control-character paths; **+2026-10-20: 71 — +5 admin per-user sessions: masked list (`total` 2, full token never in the payload), admin-only read, bulk revoke `{"revoked": 2}` → list now 0, revoke is admin-only, unauthenticated refused; +5 account expiry: set/read-back/clear, past date and garbage refused, admin-only, an expired account cannot authenticate (`401 ... expired`), the sweep deletes only the expired account's sessions**) + `quota_integration.rs` (**8**, **2026-10-22** — the truthful home-quota contract: a stored limit is answered `enforced: false` with a `reason_code` from the localisable whitelist plus a non-empty diagnostic `reason` (never the saved value echoed as success), an uninspectable path is deterministically `inspect_failed`, a home that is absent or disabled yields `quota: null`, an unlimited home reports the accounting state, and the auth/negative-size guards hold) | [x] `users.spec.ts` (**94 `test(` / 96 runtime** — **+2026-10-22 (ROADMAP 3.1–3.3): §18 home-quota state, 8 tests** — 18.1 real server: a member with no home folder gets `quota: null`; 18.2/18.3 stubbed `GET .../home`: the card renders **NOT enforced + localised reason + read-back usage** and **enforced on <mount> + used bytes**, never both; 18.4 unlimited → accounting wording, no phantom limit; 18.5 unknown `reason_code` → falls back to the server detail instead of a blank card; 18.6 French card localises the reason and leaks no raw key nor the English sentence; 18.7 `used_bytes: null` renders no usage figure; 18.8 a successful `PUT` followed by a still-unenforced `GET` keeps "NOT enforced" (no false green). **82** before — **+2026-10-21: 5.10b** pins the host-account split from the UI side: a HomeVault account carries no Linux-account badge, no refusal hint, says "Deleting this account also removes its Linux user." and keeps Delete enabled; **81** before — recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; `npx playwright test --list` remains the reference and has not been re-run), incl. transfer quotas + §16: slider offered for an admin too (16.4 inverted), folder picker opens/closes (16.6) + 4.11 `homevault_admin` chip read-only derived from the role — §16 verified live via Playwright MCP on 2026-09-08; the `beforeAll` of the whole file requires a NOPASSWD sudo for `groupadd`/`useradd`, absent here) ; **+2026-10-20: account expiry (migration 050) — new self-contained spec `tests/e2e/users/account-expiry.spec.ts` (4) green 4/4 (Firefox, scratch server :8099): "No expiry" on a fresh account with no badge, a `datetime-local` horizon persisted as the matching UTC instant (verified through `GET /api/users`), removal clearing it to `null`, and the API refusing a past horizon and a member caller. It caught a duplicate `users-ok` testid (the card's notice collided with the pane-level one) — the card now owns `users-expiry-ok`/`users-expiry-err`** ; **+2026-10-20: admin per-user sessions — new self-contained spec `tests/e2e/users/admin-sessions.spec.ts` (2) green 2/2 (Firefox, scratch server :8099): the detail pane's Security card lists the target's sessions masked to `…` references (the full shared token never reaches the DOM), "Sign out everywhere" empties the list, the server-side truth reads total 0 and the member still signs back in; done OUTSIDE the main users.spec.ts because file's beforeAll needs NOPASSWD sudoabsent here — the sub-spec creates its own throwaway member only; it caught a real spec bug (the init-script payload destructure mismatched, the admin session was never seeded) and drove a UI-behaviour fix: the sessions list reload guard (`sessionsFor` memo, refreshed on open/switch only) so the post-save rail refresh no longer wipes the just-set success notice** ; **+2026-09-12: Appliance design migration — `PageLayout`, grid/thumbnail view + settings sheet + creation in Tailwind-on-tokens, `FolderPicker`/`Modal` kept, selectors `data-testid="users-*"`, zero scoped CSS; the pre-existing TS error (`selectedUser`→`never` reduction in `{:else}`) fixed** ; **+2026-09-13: persistent 2-pane master-detail (Aurora Mail pattern) — grid/thumbnails on the left (360px column), sheet/creation on the right; the list is removed from the DOM when the container goes below 760px AND a detail is open (ResizeObserver on the split, not a viewport media query — the window floats), test 7.4 "one view at a time" preserved** ; **+2026-09-14: Figma mockup redesign (`4pwNRZU5wQjSWQBODFViKz` node `3:99`) — account rail on the left (New user button, search, 4 filters, counter, compact rows avatar/name/badge/state, footer "Quotas view" → Quotas app via `hv-open-app`) + detail pane in **6 `SectionCard divider` cards** at content height (Identity / Role and groups / Home folder / Transfer quotas / Security / Actions); the fixed heights of the mockup (152.69 / 216.23 / 173.09 / 887.26 px) are not copied; the switch threshold goes to ≈ 1065px window width (760px measured on the content column: rail 257px + `p-6` 48px already deducted); **pre-existing bug `canBack` FIXED** — the variable was declared nowhere and the shipped bundle raised a `ReferenceError` in strict mode when opening the creation pane (the "New user" button was broken in production); the notion is removed, not declared (the back bar is always rendered when a pane is open). **Spec not modified and not re-run** ; **2026-09-14 (same day): `npm run check` → 7 errors, none in `Users.svelte`/`Quotas.svelte` (the 7 are pre-existing: `Restic.svelte` 2, `Syncthing.svelte` 2, `Downloader.svelte` 3); `npm run build` OK (32 s, `Users-CCVDHXRe.js` + `Quotas-DbeDd7_t.js` emitted); `cargo build` OK (9 min 55, no warning). The check found and had 2 real defects of `Quotas.svelte` fixed: `t` never imported from `$lib/stores/i18n` (22 cascading errors) and an undeclared `{subtitle}`. The E2E pane remains not run** ; **+2026-09-14: standardization of headers — 1.2 no longer expects the recap "%accounts% accounts · %locked% locked" (`users.summary`) in the subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the counters remain visible, in the filter chips (the test verifies the 4 `[data-testid="users-filter"]` so that the reading does not disappear silently). Number of tests unchanged (81, recounted on 2026-09-14); spec modified but NOT re-run** |
| 6 | Quotas | [x] **2026-10-07 — verified live** (Firefox scratch server, `PORT=8099` + isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup` as in CI, no real data): opening from the launcher, header [ICON] [TITLE] only, the 3 filter chips (All/Users/Groups) `role="group"` + mutually exclusive `aria-label` (`aria-pressed`), seeded row "USER" + name + "Updated on …", measured `used/limit` bar (50 MB of 100 MB → `aria-valuenow="50"`, `aria-valuemax` 100, track filled at 50%) and "Unlimited" on the disabled direction, "Manage in Users" emits `hv-open-app` (`detail.id='users'`) without leaving `/desktop` or closing Quotas, console without errors. **Compilation acquired on 2026-09-14**: `npm run check` 0 errors (2 real defects fixed: `t` not imported, `{subtitle}` not declared), `npm run build` emits `static/dist/assets/Quotas-*.js`, `cargo build` OK. Detail in the 2026-10-07 addendum of `CLAUDE.md`. | n/a (screen with no backend of its own — reads `GET /transfer-quotas/{type}`) | [x] **`tests/quotas_integration.rs` (15, 2026-10-23)** — the gap is closed: the list endpoint's admin gate, the self-or-admin single read, the period round-trip and the window settlement are asserted against the assembled router (the file was recreated after its 2026-09-09 deletion; see the Transfer quotas sub-feature row) | [x] `tests/e2e/quotas/quotas.spec.ts` (**41 `test(` / 43 runtime**; **38/38 green 2026-10-07** (Firefox, scratch server; reference count `npx playwright test --list --project=firefox` = **38**, `grep -cE '^\s*test\('` = **36**, the difference coming from the responsive loop 360/768/1920, 1 line → 3 tests). Read-only: loading, all/users/groups filters, "Unlimited" row when the direction is disabled, progress bar = `used/limit`, "Manage in Users" (`hv-open-app`), §10 security (no write, no token in the DOM/URL), edge cases (500 → error state, empty backend → empty state), console without errors, memory regression. **No dead selector found** — the spec postdates the Appliance migration of 2026-09-12 and already targets the `data-testid`, it passed as is. **+2026-10-23 (ROADMAP 23)**: 35 → **40** `test(` — new §15 renders the counting period and its reset date per row, re-reads the period/instants from the API, checks a lifetime row carries no reset, and asserts the read scoping at the API level (a member is refused the list and another user's quota, and can still read their own). The seed row is now a **monthly** quota so §15 has a window to show, and `afterAll` restores the period too. Written, not run. |
| 7 | Groups | [x] 2026-07-13 (+07-18, 07-21) | [x] `groups.rs` (**9** — +4 on 2026-10-21: `normalize_member_list` trim/dedupe, blank-name rejection, the `MAX_GROUP_MEMBERS` (500) cap, empty list allowed) | [x] `groups_integration.rs` (**38** — **+2026-10-21**: a member that is not a HomeVault user is refused on create and on add-member (before any host command), a blank member name is refused, and `purge_group_references` drops all four display-name references (`hv_groups`, `transfer_quotas`, `resource_group_shares`, `share_permissions`); 34 before — **+2026-10-20: 8 rename/description/delete** (`PUT /groups/{name}`): no token, member caller refused, the admin group cannot be renamed (sudoers rule), a rename without sudo is a clean 400 and leaves the mirror untouched, description stored/updated/cleared through the mirror contract, and a rename moving the dependent rows (`hv_groups` name + `full_name`, `resource_group_shares.group_name` — old name empty, new name holds the grant, `gid`/description preserved)) | [x] `groups.spec.ts` (61 `test(` / 63 runtime — **+2026-09-23: 16.1 avatar contrast guard** (white initials ≥4.5:1 on the 6 stops of `$lib/utils/avatars.ts`, row tiles + detail); 60 before this addition to the grep, **63 at `--list`** (2 tests generated in a loop); recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; incl. transfer quotas) ; **+2026-09-12: Appliance design migration — `PageLayout` (list rail / detail panel), `SectionCard`, `FormRow`, Tailwind-on-tokens, zero scoped CSS, selectors `data-testid="groups-*"`; the 860px breakpoint (stacking of the panels) is driven by `matchMedia` → `sidebarHidden`** |
| 8 | Dashboard | [x] 2026-07-13 (+07-15, 09-02) | [x] `handlers/dashboard.rs` (**40** — `format_uptime` (6) + **+2026-09-07** `format_bytes`, `parse_size` (incl. fraction + locale comma, **bug fixed**), `calculate_individual_disk_score`, `calculate_disk_summary`, `calculate_disk_score`; **+2026-10-23: 23 → 40, ROADMAP 15** — the 4 disk-score cases were rewritten for the reweighted SMART 40 / temp 20 / usage 40 scoring (the hardcoded `error_score = 10` is gone: a failed disk now scores 0, a perfect one 100) plus `test_individual_disk_score_has_no_unmeasured_component`, `health_status_for_score` (2 — the documented `healthy|warning|unhealthy` vocabulary, never `critical`), `mean_core_usage`, `snapshot_is_fresh` (the 30 s disk-health cache window, backwards clock included), 10 widget-validation cases (`validate_add_widget`: slug charset, over-long id, blank / over-long / control-char title, `size` matrix, grid position and span bounds) and 3 preference cases (`validate_widget_preference`: unsafe key, bounded panel offset)) | [x] `dashboard_integration.rs` (**19**, +2026-09-07 — auth split on the 15 read endpoints + widget-CRUD + widget-preferences, widget add/list/reorder/remove cycle, per-user preferences upsert + partitioning ; **+2026-10-23: 10 → 19, ROADMAP 15** — per-account scoping (a second account sees nothing, cannot delete or reorder the first one's tile, and may reuse the same id; a delete leaves the other account intact), every row is attributed to the session owner, a foreign / unknown id is `404` (not `403`), the field-validation matrix (unsafe and over-long id, blank / over-long title, bad and oversized `size`, row 0, col off the grid, span too large) leaves the table empty while a valid add still trims the title, reorder rejects a non-slug id and 65 ids and treats an empty list as a `200` no-op, the 64-tile cap is per account, an **expired** session on widget-preferences is `401` **and writes no row**, a bogus token is `401` on both methods, `/dashboard/widget/memory` reports self-consistent `/proc/meminfo` numbers, and an unknown widget id is `404` instead of `200` with a placeholder type) | [x] `dashboard.spec.ts` (**48 runtime / 46 `test(`**, 12 categories — recounted on 2026-10-23 with `npx playwright test --list --project=firefox` **and** the file's `grep -cE '^\s*test\('`, the two differing by the §5 responsive loop ; **+2026-10-23 (ROADMAP 15): 2.8** the health hint follows the real `data-health-status` instead of the constant "ok" text, **2.9** the SMART footer prints the healthy/total count and never the literal "last test 2", **2.10** no fabricated `UP TO DATE` badge, **3.6** a hidden tab issues no `server-details` request for a full interval and refreshes on `visibilitychange`, **4.6** a throwaway non-admin sees the admin-only notice instead of the app-usage card and makes no `/docker/containers` request. Previously 37 runtime tests, 12 categories — recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; the entry said 39, an outdated number, and the "39/39 green" of 2026-09-07 therefore concerns an earlier state; **+2026-09-14: standardization of headers — 1.7 and 7.1 no longer expect the "Live" badge (decorative element removed with the subtitle) but verify `.hv-winhdr-title` alone, `.hv-winhdr-subtitle` and `[data-testid="dashboard-live"]` at `toHaveCount(0)`**; read-only, the screen has no mutating action) ; **+2026-09-12: Appliance design migration — `PageLayout`, 12-col bento-grid via Tailwind container queries, SVG (gauge/core/mem) kept, selectors `data-testid="dashboard-*"`, zero scoped CSS** |
| 9 | Storage | [x] 2026-07-17 (+09-06 test suites + "Virtual disks" re-scope; +09-07 `impeccable` layout redesign; **+2026-09-07 real test: (a) list/scrub/destroy a real mdadm + ZFS virtual disk — `PoolService::scan_pools` fixed (device_count + mount_point + usage via `df`); (b) full LUKS cycle create/lock/unlock/delete on `/dev/loopN` + real `cryptsetup` — 2 frozen bugs fixed: `handle_encryption_create` did not validate `req.device` (now guarded on `get_available_devices` `!in_use`), `encryption::erase` left the LUKS header (+ `wipefs -a`)**) | [x] `pools.rs` (35), `cache.rs` (**34** — +21 on 2026-10-22 (ROADMAP 2): `parse_df_space`/`sum_sizes`, `PoolMounts` resolution, `MoverResult::status`/`failed_shares`/`outcome_records`, and the whole data-movement path against real temp dirs with real `rsync`/`find`/`df` — a normal move, an empty share, an unmounted target, a failing share followed by a succeeding one (partial), and a destination too small for the payload (refused, nothing deleted, nothing written)), `cache_scheduler.rs` (**4** — the failed-run retry backoff), `encryption.rs` (**24** — +22 on 2026-10-23 (ROADMAP 9): the passphrase policy (`validate_passphrase`: 12 characters, no wordlist entry, not the label, four distinct characters), `change_key_stdin` (old + new twice, the three lines `luksChangeKey` prompts for), the erasure gate (`erase_gate` + `ReleaseBlocker`), the `/proc/self/mounts` parsing (`is_mounted_in`/`unescape_mount_field`, including the `\040` escapes and a sibling mount point that must not match), the fstab pure edits (`with_marked_entry`/`without_marked_entry`), and `run_timed` (a fast command returns its output, stdin is fed to the child, a hung command is killed at the deadline, a non-zero exit keeps the tool's own stderr) — 2 → 24), `handlers/encryption.rs` (**6** — new on 2026-10-23 (ROADMAP 9): the label and filesystem validators, the mount point derived from the label, and both branches of the unlock rollback message), `snapshots.rs` (**6** — +3 on 2026-10-22 (ROADMAP 5): the backend follows the pool's own type instead of the installed binaries, mdadm pools are resolved by the filesystem on top, and the `/proc/mounts` fstype lookup), `handlers/storage.rs` (**9** — `vd_name`, +2026-09-07; **+2026-10-23 (ROADMAP 7.2): +4 — `md_device_candidate` keeps a `/dev` path and expands a bare name, and `mdadm_check_command` runs `sudo mdadm --action check <dev>` with no `-c`/`bash`/`sh` argument, so an mdadm "Scrub" can never fall back to a shell**; the count was 3 in this entry while the file already held 5 `scrub_in_progress` cases), `handlers/cache.rs` (**12** — `validate_cron`/`validate_min_free_percent`/`validate_share_name`/`validate_target_pool`/`normalize_pool_field`) | [x] `storage_integration.rs` (**32**, +2026-09-07 ; **+2026-11-05 (P1-13): the Btrfs layout/rollback routes are in the auth matrix and one test proves the gates run before any mount/subvolume command**; **2026-10-22: `min_free_percent` is now 1–95, so the out-of-range test gained the `0` case**; **2026-10-23 (ROADMAP 9): the encryption write matrix gained `POST /api/encryption/volumes/{id}/passphrase` and every encryption passphrase moved to the 12-character policy — the short-passphrase test now names "at least 12 characters"**), `encryption_integration.rs` (**15** — new on 2026-10-23 (ROADMAP 9), in-memory SQLite + assembled router: the read/write auth split on every route including the new rotation one, the passphrase policy (11 characters, a wordlist entry, and a compliant payload that must get *past* the policy to the device gate), the duplicate-label gate, `DELETE` refusing to erase a volume it could not release **and keeping the metadata row** (the observable form of 9.2), `DELETE` still requiring confirmation, lock/unlock reporting a failure instead of success, the three rotation refusals (weak new passphrase, reuse of the current one, unknown volume) plus a clean failure on a missing device, and the list reporting a stored volume as `unlocked: false` — no real block device is touched, the opt-in E2E owns that), `cache_integration.rs` (**11** — 2026-10-22 (ROADMAP 2): auth split on every cache route, cron/min_free/target-pool rejections, `cache_pool: ""` clearing the designation, a failed run recorded and reported, the stale `running` row closed after a restart, the per-share history round-trip and the empty `shares: []` of a pre-052 run) — includes `GET/DELETE /api/storage/virtual-disks[/{name}]` + `/scrub` + **`POST /api/storage/virtual-disks/{name}/grow` (router + write-matrix entry, 2026-10-19)** (auth split) + `test_encryption_create_rejects_a_device_that_is_not_available` (guards `get_available_devices` before `cryptsetup`); still no real virtual disk / LUKS volume via integration; **+2026-10-10: auth guard strengthened on `POST /storage/disks/{name}/smart/test` — `require_auth` → `require_admin` (launches a real `smartctl -t` on the host) + trace in the event journal; read/write matrix extended to the 3 SMART endpoints** | [x] `storage.spec.ts` (**40 `test(` / 48 at `--list`**, 41 passed + 3 opt-in skipped on 2026-10-23 — **2026-10-23 (ROADMAP 9): 3.5 now pins the passphrase policy (the "12 characters" floor named, a wordlist entry refused, and a compliant payload reaching the device gate with no passphrase complaint) and 3.8 covers the new rotation route on an unknown volume; 13.1 follows the create job (`{job_id}` + `/api/progress/{id}/status`) instead of reading an id off the response, then rotates the passphrase and proves the rotated-away one no longer unlocks**; +2 in §12 "Cache mover history": a stubbed `partial` run renders its status badge, both share names and the failure reason, and a run without per-share rows still renders; 3.2 now also rejects `min_free_percent: 0` and 3.2b pins "an empty `cache_pool` clears the designation"; `gotoDesktop` gained the shared `dismissWhatsNew` helper — without it the once-per-version "What's New" overlay swallowed the first dock click of every fresh run) — the "Virtual disks" tab is covered (nav + count of 6 tabs); **34/34 green 2026-09-07**; no listing/destruction of a real virtual disk; **+2026-09-12: Appliance design migration — `PageLayout` (6 tabs), Tailwind-on-tokens cards, `Modal`/`Toggle`/`Alert` kept, selectors `data-testid="storage-*"`, zero scoped CSS**; **+2026-11-05 (P1-11): 14.6 and 14.7 mock one mdadm array and its level plan — the backup checkbox is required before the action enables and the exact `{target_level, backup_confirmed:true}` payload is sent; an array with no allowed conversion shows its reasons and cannot submit**; **+2026-11-05 (P1-13): 14.8 mocks a legacy Btrfs pool and proves the migration is only posted after the typed pool name; 14.9 mocks a migrated pool + snapshot list and proves the rollback payload `{snapshot, confirm:true}` is only sent after the typed snapshot name** |
| 10 | RAID | [x] 2026-07-13 (+09-05 creation-only re-scope; **+09-07 layout redesign via `impeccable` — 3-step wizard re-skinned mono + `--hv-*` tokens, 3-dot stepper, per-level disk diagram, touch targets; markup/CSS only; `svelte-check`/`vite`/`cargo` OK; verified live step 1**; **+2026-09-07 real test on 6 loop devices: end-to-end mdadm + native Btrfs + ZFS creation/scrub/destruction via the API; 4 bugs fixed — non-`sudo` `mount`/`umount`/`blkid` in `disk.rs`, argument order `zpool create` (name before the vdev type → all ZFS creation failed), and the wrong disk count / mount point in `scan_pools` for mdadm+ZFS**; **+2026-10-19: `get_available_devices` was broken — `in_use = role != "Available"` against a role string `detect_disk_role` can never produce, every device reported `inUse: true` and the wizard's step 2 was permanently empty (verified live against this machine's disks via a compiled probe); rewritten facts-based (`gather_device_facts` + pure `facts_indicate_in_use`): partitions / filesystem signature / mount points / mdadm superblock / ZFS / Btrfs membership / system disk (whose raw `df /` path is now matched symmetrically — the old trailing-digit trim never matched a partition of an nvme system disk); the same gate moved into `handle_raid_create` as `validate_pool_devices` so a hand-built admin request can no longer `mdadm --create --run` over a mounted/system disk**) | [x] `disk.rs` (**72** — create_raid validation + `parse_size` + `is_pseudo_disk_name` (`zram*`/`ram*`/`mtdblock*`) + `read_sysfs_mmc_identity_from` (`mmcblk*` SD/eMMC sysfs model lookup), **+2026-10-19: +5 `facts_indicate_in_use` (all-clear = free; each of the 7 signals disqualifies), `same_device`/`is_same_disk` (partition-vs-disk: `sda1` of `sda`, `nvme0n1p2` of `nvme0n1`, lookalikes `sdaa`/`nvme1n1` rejected), `in_use_reason`**; **+2026-10-19 (20 points): +8 — `validate_virtual_disk_request` (baseline request accepted; unknown filesystem; spares don't count toward the level minimum; spares/missing refused on native ZFS/Btrfs; degraded-create bounds = level's missing budget and exact-minimum layouts; uneven raid10 refused on mdadm and ZFS), `friendly_tool_error` (known stderr patterns mapped to sentences, empty stderr, last error line kept), `SubProgress` (0-100 window mapped into the parent job; the 6 `create_raid` tests now drive the `MdadmRaidRequest` signature)**; **+2026-10-23 (ROADMAP 7.3): +9 — the pure halves of `delete_raid`'s unmount gate: `mount_targets_matching` (the kernel-node spelling `/dev/md127` vs the `/dev/md/<name>` symlink the caller passes, a literal device match, the `UUID=…` resolver, unrelated filesystems ignored, an octal-escaped mount point) and `not_mounted_error` (only the "nothing was mounted" wording is tolerated — a busy or failed unmount must abort the teardown), plus `unescape_mount_field` (`\040`/`\011`/`\012` decoded, plain and broken escapes left alone) — 25 → 34**; **+2026-10-23 (ROADMAP 7.4): +8 — `is_removable_transport` no longer treats `ata`/`sata`/`scsi`/`nvme` as removable (every internal disk was listed as removable media), `lsblk -P` parsing keeps a label containing a space and reports an unmounted/unlabelled device's empty fields as empty, `parse_smart_attribute` reads the standard row, survives a two-token attribute name (the `parts[9]` regression that read WHEN_FAILED as RAW_VALUE) and ignores header/note lines, and `parse_smart_polling_minutes` picks the Short/Extended/Conveyance duration instead of always returning the Short one, and a RAW_VALUE carrying a `(Min/Max 20/45)` suffix still reads its number — 34 → 42**; **+2026-10-23 (ROADMAP 8): +21 — the create path's safety contract, all pure: `parse_mdadm_detail` (standard body; a `removed`/`missing` slot never becomes a member; no `Name :` line → `None`), `plan_teardown` ×6 (wipes only the intersection of request × current members; foreign UUID; renamed node; unknown identity → stop-without-wipe; array already gone; label fallback when no UUID was printed), `wipe_targets` (basename match, a by-id path stays untouched), `classify_array_state` (`readonly`/`read-write` usable, `clear` = no array), `resync_percent` (only for a running resync, never above 100 %), four `wait_for_array_state` cases against a temporary sysfs (becomes `active`; resync percent mapped into the 10-40 band; `clear` fails fast instead of polling the deadline; a silent kernel times out instead of continuing), `run_timed` (fast command; a hung command is killed at the deadline), the single degraded-create policy table + its refusal wording, and a test proving the API gate and the job return the same refusal, plus three `zfs_create_args` cases pinning the argv that once broke ZFS creation live (pool name before the vdev type, **no `-f`**, one `mirror` group per pair for raid10, bare names expanded to `/dev/…`) — 42 → 66**; **+2026-11-05 (P1-9): +4 — ZFS `spare` group accepted, Btrfs spare refused with the alternative named, raidz3 ZFS-only + four-member minimum, and the argv maps `raidz3` and appends one trailing `spare` group (raid10 mirrors included) — 72 → 76**); **+2026-11-05 (P1-11): +6 — the in-place level-change contract: the documented transition subset (0→5/10, 1→5, 5↔6), refusals with their reason (raid0→raid1 layout, raid0/raid1→raid6 two-step, raid10, already-current, unknown), target minimums (raid1 2→raid5 needs a third disk, raid5 3→raid6 needs four) and even-member RAID 10, the complete option list (every refusal explainable), the backup path confined under `DATA_DIRECTORY` and a hostile name stripped, and `mdadm --detail` parsing of level/count/reshape — 76 → 82**)  | [x] `raid_integration.rs` (6, +2026-09-07 — auth guards on the `/api/storage/raid*` endpoints **+ the new `POST /api/storage/virtual-disks/{name}/grow` (router + write-matrix entry, 2026-10-19)**: read = session, create/rebuild/grow = admin (the legacy `DELETE /api/storage/raid/{name}` left the matrix with the route on 2026-10-23 — destroy lives at `DELETE /api/storage/virtual-disks/{name}`, covered by `storage_integration.rs`); `require_admin` rejects **before** the handler body so `mdadm` is never reached; **+2026-10-23 (ROADMAP 8): +1 — the only creation-path assertion that runs without hardware: `POST /api/storage/raid` with a degraded raid0 and a degraded raid10 answers 400 naming the reason ("no redundancy" / "explicit layout") and starts no job, which also proves the refusal happens before the device gate**); **+2026-11-05 (P1-9): +3 — raidz3 on ext4/xfs/btrfs is a 400 naming "ZFS layout" and starts no job, a ZFS raidz3 + spare passes the shape gate and stops at the device gate, and Btrfs spares name the alternative**; **+2026-11-05 (P1-11): +1 — the level-change plan is `401` without a session, an unknown target (`raidz3`) and a missing `backup_confirmed` are refused `400` before any scan/command, the migrate route joined the admin write matrix, and the plan of a missing array is a clean `400`, never a `500`**; still no real creation/destruction (host required) | [x] `raid.spec.ts` (43 `test(` / **45 runtime**) — **35 passed / 8 skipped 2026-09-07**; real opt-in cycle `RAID_E2E_DEVICES` (create in RAID → see in Storage → destroy); **+2026-10-19: the disk-step tests (2.3/2.4/3.2/4.2/5.1/5.2/8.1) no longer skip silently — `requireFreeDisks` turns a skip into a hard failure on an opt-in run, because silent skipping is exactly what kept the "every disk inUse: true" bug green; +13.2 (the provided devices must appear as selectable step-2 rows in the UI) and +13.3 (once an array exists, its devices are reported `inUse: true` and a second create on them is refused 400 by the server-side gate)**; **+2026-10-19 (20 points): creation is a backend tracked job (POST answers `{job_id}` before the array exists) — 13.1/13.3 now poll `/api/storage/virtual-disks` for the new array (30 s ceiling) instead of racing the response; 14.2's fake-device rejection still hits the synchronous 400 gate before any job starts**; **+2026-09-12: Appliance design migration — `PageLayout` + 3-step wizard in Tailwind-on-tokens (stepper, disk diagrams, capacity, recap), selectors `data-testid="raid-*"`, zero scoped CSS**; **+2026-11-05 (P1-9): 2.1 now expects 6 levels including RAID Z3 (four-disk minimum), 14.4 refuses raidz3+ext4 and accepts a clean rejection for raidz3+zfs — 44 `test(` / 46 runtime** |
| 11 | Health | [x] 2026-07-16 (**+2026-09-07 complete test suites — the last screen without coverage**; **+2026-10-22 ROADMAP-1 pass — the 5 findings fixed, see the Summary addendum of that date**; **+2026-10-24: every host probe bounded (ROADMAP X.2) — see the 2026-10-24 addendum**; **+2026-10-31: Security Advisor card (QW-10 / DSM-18) added to the screen — see the 2026-10-31 addendum**) | [x] `src/health.rs` (**22**, +2026-09-07 — `determine_overall_status` precedence (incl. the empty case vacuously-`all()`), PascalCase serde of `HealthStatus` (the handler's lowercase mapping depends on it), `check_storage` Healthy/Warning depending on `DATA_DIRECTORY`, bounded `check_memory` %, `check_network_interfaces` always lists `lo`; **+2026-10-19: the aggregate became 9 checks** — `raid_arrays` degraded → Warning, failed/inactive → Error, nothing defined → Healthy, mdadm unavailable → Unknown; **+2026-10-22 ROADMAP-1: `run_all_checks`/`run_check` went async + self-offloading (`spawn_blocking` inside the service), so the DB-dependent tests moved to `health_integration.rs`** and the remaining 17 unit tests cover the pure contracts — `degrade` passes a check through / turns any `Err` into a well-formed `Unknown` slot ("Check unavailable: …", slot name kept), `run_system_check` rejects an unknown name, `link_is_up` operstate matrix (`up` is the only up-state, loopback special-cased, an unreadable operstate never counts as up), `check_network_interfaces` lists `lo` + carries `operstate` in every details entry, `parse_cgroup_unit` slice-path / non-service units / any slice depth, `service_names` adds the derived own unit without duplicates; **+2026-10-24: +5 deadlines (ROADMAP X.2)** — source guard, kill path, exit-as-data, missing tool, probe-budget ordering) + **`src/smart.rs` (2, 2026-10-22)** — the shared sudo-wrapped `smartctl` argv builder (`sudo smartctl <args> <device>`, std and tokio variants agree) + **`src/security_advisor.rs` (27, 2026-10-31, QW-10)** — every one of the 19 findings in both directions (clean box = 100 / nothing actionable; admins without 2FA, the default account name, a pending forced password change, HTTPS off, expired vs expiring vs valid certificate, a self-signed certificate as `info`, the `tls-*`/`firewall-rules`/`smb-*` checks correctly *skipped* when their premise is absent, a missing reachability rule as `fail`, guest access and SMB1 as `fail` vs signing as `warn`, world-writable shares, quotas as `info`, missing snapshots / config backup, the optional auto-block, an available update), the three design rules (an unreadable signal is never a `pass` and always says where to look; the score is the weighted percentage **floored**; a warning costs half a point; findings are worst-first and stable within a severity), the severity weights and ordering, and the four collection helpers (`is_default_account_name`, `allowed_ports` over the ufw `22/tcp` / `8080` / range / empty shapes, `mandatory_rules_present` for SSH + the web port, `smb_signing_required` on the exact directive) | [x] `health_integration.rs` (**16**, +2026-09-07 — `GET /api/health` **always responds 200** and reports the auth failure in the body (`success:false`, no `data`); valid **non-admin** session → `success:true`; **9** named checks (`EXPECTED_CHECKS` grew 8 → 9 with `raid_arrays`, 2026-10-19) + valid lowercase statuses + non-empty messages; `overall_status` = `status`; no leak of PascalCase status; aggregate consistent with its checks; `cpu_usage`/`memory_usage` ∈ 0..=100; never a 500; no credential/hash/token returned; **+2026-10-22 ROADMAP-1, 5 new**: the nine checks run **in order with `authentication` first** (the order test moved here, `test_all_checks_run_in_order_authentication_first`), every `run_check` arm dispatches against a real pool, `check_authentication` performs a **real account-table read** (`details.accounts` is an i64), and — via an `ALTER TABLE users RENAME TO users_parked` — one unreadable check **degrades to `unknown`** ("Check unavailable") while the other eight still run and the endpoint **still answers `success:true`** with all nine slots and a consistent aggregate; **+2026-10-24: +1 slow-probe (ROADMAP X.2)** — a shim `df` sleeping 250 ms is waited out, the endpoint still answers `200`) + **`security_advisor_integration.rs` (10, 2026-10-31, QW-10)** — the admin matrix on `GET /api/security/advisor` (`401` with no token, `401` with an unknown token, `403` for a signed-in non-admin), the complete report on a fresh box (score ≤ 100, `app_version`, RFC 3339 stamp, ≥ 10 findings, counts summing to `total`, findings sorted worst-first), every finding carrying a known severity + a title + evidence and a `fix` whenever it is actionable, the default-account-name finding present for `admin` and absent for `owner`, an admin without 2FA as `warn` **clearing to `pass` on the very next call** once the `user_2fa` row exists (the acceptance criterion — no restart, no cache), a plain member without 2FA **not** turning that check into a finding, the config-backup finding going `warn` → `pass` when a `.hvbak` lands in `data/config-backups/` and back to `warn` for a non-archive file, the unreadable-signal rule (no default `pass`), and a payload that contains no `argon2` / `totp_secret` / `password_hash` / `csrf` string. Each test runs on a **current-thread** runtime built inside a plain `#[test]` — the `ENV_LOCK` guard is not `Send`, so a `#[tokio::test]` body holding it across an `await` hangs the multi-threaded flavor; the sessions are inserted directly instead of going through Argon2 | [x] `health.spec.ts` (35 `test(` / 37 runtime tests, 12 categories — **36/36 green 2026-09-07**; read-only; +2026-09-08: manual Refresh button removed, auto revalidation on focus (`onAppVisible`); opening via the launcher + via the dock "Security" icon (`handleDockAppClick('health')`, historical mislabel, asserted); 8 cards; the global badge follows the worst check (read from the raw API); Refresh is GET-only without its own polling; 500/malformed payload → empty state, never a white window *—"empty state on failure" superseded on 2026-10-22, see 11.1 below*) ; **+2026-09-12: Appliance design migration — `PageLayout` + `EmptyState`, Tailwind-on-tokens, zero scoped CSS, selectors on `data-testid="health-*"`** ; **+2026-09-14: standardization of headers + moving a real reading** — 1.3 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the global status is no longer a header badge but the `data-testid="health-overall"` band at the top of the body, which keeps the former `data-testid="health-badge"`: 1.5 therefore verifies the band **and** the badge. The label goes through the renamed i18n key `health.overall_status` (ex-`health.subtitle`, now orphaned) — renaming rather than deletion to keep the parity of the 4 locales. Number of tests unchanged; **spec modified but NOT re-run** ; **+2026-10-22 ROADMAP-1: 11.1 rewritten — a failing payload now surfaces the explicit error state (`data-testid="health-error"`) with a Retry button (`health-retry`), never the empty state and never a blank window** (error state visible, retry visible, `health-empty`/`health-card`/`health-loading` = 0), and **new 11.3 — retrying against the real backend recovers** (unroute → click retry → 9 cards + the overall band back, error state gone). **Spec re-run green 2026-10-22: 37/37 on Firefox AND 37/37 on Chromium** against the rebuilt binary on a scratch server (`PORT=8099`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup`) — including the two new/rewritten edge cases; **+2026-10-31 (QW-10): §13 Security Advisor, 10 tests** — the card renders above the checks with a real `role="progressbar"` score (0–100) in DOM order before the cards; the summary/score match the raw `/api/security/advisor` payload and every finding carries a known severity; actionable findings list their severity + fix + a deep-link button, or the card says there is nothing to fix; the full checklist is a closed native `<details>` that opens on click; "Re-check" issues one more request and never coexists with an error state; a failing call surfaces `advisor-error` and **drops the stale score** while the health cards stay; the DOM/storage/URL contain no credential; a deep link dispatches `hv-open-app` with `source: "security-advisor"` (and the Settings category when it targets Settings); the card holds no horizontal overflow at 360/768/1920 px; the deep-link button is keyboard-activatable. `health.spec.ts` goes 35 → **45 `test(`** (47 runtime) and the whole file is green: **47/47 on Firefox**, 2026-10-31, rebuilt binary on a scratch server (`PORT=8097`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup`) |
| 12 | Terminal | [x] 2026-07-13 (+ ~6 follow-ups up to 09-02, +09-06 Rust tests; **+2026-10-20: full suite re-run green against a production bundle — the four "pre-existing" failures were ornamental (see the addendum above); 81/81 on firefox AND chromium**) | [x] `handlers/terminal_pty.rs` (13 — `is_valid_tab_id`, `resolve_tab_id`, `generate_ticket`, `PtyManager` ticket cycle, `kill_session` purges the tickets, cleanup no-ops) | [x] `terminal_integration.rs` (10 — admin auth matrix on `/ticket` + `/close` + `/cwd` + `/ls`, tab id validation, `close_tab` no-op) | [x] `terminal.spec.ts` (**77 `test(` / 81 runtime tests**, recounted on 2026-09-17 via `npx playwright test tests/e2e/terminal/terminal.spec.ts --list --project=firefox` — 77 by `grep -cE '^\s*test\('`, the difference of 4 coming from the tests parameterized in a loop; the entry said 74 before 2026-09-16) ; **+2026-09-17: `terminal-tabs` no longer scrolls vertically (`overflow-y-hidden` — a lone `overflow-x: auto` computes `overflow-y` as `auto`, and the 1px that the `min-h-11` leaves under its content box was enough; test 2.9) and `terminal-body` paints the same background as the xterm theme (test 2.10 — the container had stayed in `#1e1e1e`, so the sub-cell remainder of the `FitAddon` painted a black band under the last line)** ; **+2026-09-12: Appliance design migration — `WindowHeader` header (and not `PageLayout`, the entry was wrong) + xterm tabs + infobar in Tailwind-on-tokens, selectors `data-testid="terminal-*"`, zero scoped CSS** ; **+2026-09-12: client completion (ghost text + clickable Tab popup, `section 19`)** ; **+2026-09-16: the tab strip leaves the `WindowHeader` — `terminal-tabs` is now a window content bar, between the header and `terminal-infobar` (new test 2.8: the strip is not a descendant of `.hv-winhdr` and its vertical order places it between the two bars); the header receives its `title`; 3.2/3.4/7.3/7.4 repaired (outdated selectors, **already red before**) ; sections 1/2/3/4/7/8/9/10/11/12/13/14/15/16/17/18 re-run on a fresh instance — all green except 4.2/10.3/19.1/19.2, **proven pre-existing by `git stash` of the modified file** (4.2 monospace family; 10.3 "< 80 requests" is an artifact of the Vite dev server, which serves non-bundled ES modules; 19.1/19.2 = completion popup, untouched area); **+2026-10-20: those four + 14.2 + 2.8 + 2.10 resolved/closed against a production bundle (`:8099` scratch server) — `dismissWhatsNew` helper added to the spec (the once-per-version modal intercepted the dock click), 2.10 repointed from `.xterm-viewport` (v5 surface, styled inline) to `.xterm-scrollable-element` (v6 surface, inline theme background — the refresh of 2026-10-19), 14.1's exact-count invariant relaxed to the survival invariants it was meant to test, 2.8 settles 400 ms before measuring. Full suite: 81/81 firefox, 81/81 chromium** |
| 13 | System journal | [x] 2026-07-18 (**+2026-09-07 real test: `system`/`syslog`/`auth`/`kern` source via real `sudo journalctl`/`tail`, unit/priority/search filters OK; 1 major bug fixed — the auto-cleanup scheduler wiped the active application log at each startup → "app" source permanently empty**) | [x] `handlers/journal.rs` (**20** — `priority_text`, `is_app_log_filename`, `text_log_path` (whitelist / anti-traversal), `strip_target_prefix`, `parse_app_log_line` (timestamp+level+target / fallback), `seconds_to_iso`; **+2026-10-23 (ROADMAP 20)**: `source_kind` (the per-source privilege level — `app` session, `system`/`syslog`/`auth`/`kern`/`daemon` admin, anything else `None`), `scan_limit` (the search window is wider than the page, hard-capped at `MAX_SCAN_LINES`), `validate_unit_filter` (real systemd unit names only, 256-char ceiling, shell metacharacters refused), `clear_outcome` (a partial clear is reported as a failure, never as done), `run_timed` ×3 (fast command output, failing exit code + stderr preserved, a hung child killed at the deadline) and `read_file_tail` ×3 (whole file when it fits, the seek-cut first line dropped, an unreadable file is an error not an empty log)) + `src/journal_cleanup_scheduler.rs` (**4** — the module's first tests, on `plan_tick`: disabled, a baseline instead of cleaning a first run, the interval boundary, and a zero interval treated as one day) | [x] `journal_integration.rs` (**17** — auth split (read=session, `clear`+`cleanup-settings` PUT=admin), `/sources` filtered per role (`app` only for a non-admin, `app`+`system` for an admin), `app` source read **for real** from `DATA_DIRECTORY/logs` (parsing + `search` filter + empty), **+2026-10-23 (ROADMAP 20)**: a non-admin is **403** on every host source (`auth`/`system`/`syslog`/`kern`/`daemon`) while still reading `app`, an admin reaches `system` (never refused by the gate, never a 500), search reaches an entry older than the page (needle on line 150 of 300 with `limit=10`), `meta.scanned`/`meta.limit`/`meta.search_truncated` when the scan window fills and `search_truncated=false` without a search, an invalid unit filter → 400 naming the accepted characters, unknown source → clean 400 `{success:false}`, `clear` deletes only `homevault.log*`, default cleanup settings (on/90d) + round-trip + `interval_days≥1` clamp) | [x] `journal.spec.ts` (**30 `test(`** / 32 runtime tests, 15 categories — **28/28 green 2026-09-07**; read-only, never empties the real logs nor durably modifies the cleanup setting) ; **+2026-09-12: Appliance design migration — `PageLayout` (filter toolbar + status footer), priorities → tokens, selectors `data-testid="journal-*"`, zero scoped CSS** ; **+2026-09-14: standardization of headers — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the real actions of the bar (auto-refresh, Clear, Cleanup settings) remain, only the decorative elements were removed. Number of tests unchanged; spec modified but NOT re-run** ; **+2026-10-23 (ROADMAP 20)**: 3.3 a bounded search says so (`meta.search_truncated` → `journal-search-truncated`, mocked route), 8.3 a failed read is an error + Retry and **never** the empty state (mocked `{success:false}`, then the real endpoint after Retry), 10.4 a non-admin session is 403 on `auth`/`system`/`syslog`, still reads `app`, gets `/sources = ["app"]`, and finds no "System Journal" tile in the launcher (created and deleted through `/api/users`). **Written, NOT executed** |
| 14 | Network | [x] 2026-07-14 (+07-15 ; **+2026-09-07: reads re-verified live; frozen `src/network.rs` fixed — *mutation* `ip`/`dhclient` wrapped in `sudo` (`ip_sudo`), `/etc/resolv.conf` + `/proc/sys/net/ipv6/*` toggles via `sudo tee` (`write_as_root`), DNS only rewrites the `nameserver` lines (`build_resolv_conf`, keeps `search`/`options`)**; **+2026-10-20: config persistence implemented (the deliberate follow-up is closed) — see the addendum above**; **+2026-10-21: persisted config is applied, verified and rolled back in the same request (production-ready) — see the 2026-10-21 addendum**; **+2026-10-24: every host command bounded (ROADMAP X.2) — see the 2026-10-24 addendum**; the READ paths below stay untouched) | [x] `network.rs` (63 — +3 `build_resolv_conf`; **+2026-10-20: +11 persistence** — netplan yaml DHCP / merged v4+v6 / mirrored renderer / bare iface key, `netplan_renderer` last-declared-wins + unknown-ignored + empty-dir (tempdir-injected), file-name namespacing, ifupdown stanzas static/dhcp/inet6 auto|static|disabled; **+2026-10-21: +9 `check_live_state_matches`** — down interface, wrong/absent static v4, DHCP any-lease, missing v4 route, multi-default-route needle, static v6 address then route, no-v6-intent unverified; **+2026-10-24: +6 deadlines (ROADMAP X.2)** — source guard, kill path, exit-as-data, missing tool, deadline ordering, privilege-helper argv) | [x] `network_integration.rs` (24) | [x] `network.spec.ts` (48 `test(` / 50 runtime ; **2026-11-06: 46 passed / 4 conditionally skipped on Firefox AND Chromium** — the last seven reds are fixed) ; standardization note: 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone (07-14 assumptions); selectors `data-testid="network-*"`, zero scoped CSS |
| 15 | VPN | [x] 2026-07-16 | [x] `vpn.rs` (47 — **+2026-11-05 (P1-17): 13 on the imported WireGuard client — allowlist parser (fields, multi-peer route merge, hook/unknown/invalid-value refusals, missing sections), canonical render/round-trip, fixed kill-switch hooks, bounded interface naming, stored-interface validation, exact argv, rollback order, confined config path** ; **+2026-10-21**: 16 on the OpenVPN profile deny list, the log parsers (`Initialization Sequence Completed` marker, fatal/transport extraction, summary timestamp stripping); **+2026-10-24: +5 deadlines (ROADMAP X.2)** — source guard (incl. the detached daemon path), kill path, exit-as-data, missing tool, deadline ordering) | [x] `vpn_integration.rs` (34 — **+2026-11-05 (P1-17): 9 on WireGuard client import/list — 401/403, empty name, invalid config, `PostUp` refusal, empty list, 404 on start/stop/update/delete, id traversal** ; **+2026-10-21**: three 400 profile-content tests, admin-guarded) | [x] `vpn.spec.ts` (44 `test(` / 46 runtime) ; **+2026-11-05 (P1-17): +5 §15 — client tab + import modal fields, import-button gating, `.conf` upload population, invalid config 400, `PostUp` 400** ; **+2026-09-12: Appliance design migration — `PageLayout` (WireGuard/OpenVPN), selectors `data-testid="vpn-*"`, zero scoped CSS** ; **+2026-10-21: production-ready pass per `CLAUDE.md`'s dated addendum — verified OpenVPN start (start → verify → rollback), profile deny list + 409 overwrite guard, `spawn_blocking` in the OpenVPN handlers, `PublicIpCache`, connecting spinner (`vpn.connecting` ×4)** ; **+2026-10-24: every host command bounded (ROADMAP X.2) — see the 2026-10-24 addendum** |
| 16 | Explorer | [x] 2026-07-14 (+07-17, +09-06: **5 robustness bugs fixed** + 8 Explorer-extra features; compiled & tested **2026-09-07**) ; **+2026-10-13 (b): Rewind (per-file ZFS snapshot timeline), server-side recents, fuzzy search, marquee-select, virtualized listing, internal drag & drop, SMB/network discovery, ext4/exfat formatting** | [x] **+2026-10-31 (QW-13 — transfer visibility): `checksum.rs` (16 — the three algorithms against their known vectors for `""`/`"abc"`, the parse matrix incl. `md5`/`crc32` refused, `hex_len`, a cumulative progress callback, `NotAFile` vs `Io`, `digests_match` case/whitespace, a `false` from the per-block callback stopping the run with no digest, the bounded `JobOutcomeStore` TTL + eviction + a clock that went backwards); `transfer_history.rs` (11 — slug round-trips, strict parses, filters that refuse an unknown value instead of unfiltering, limit clamping, the hourly prune guard, the 90-day cutoff, the builder); `upload_session.rs` (5 — idle-based expiry, a backwards clock, progress clamped to 100, ETA from the last chunk); `handlers/transfers.rs` (5 — `zone_for_kind` per kind and `None` for a non-Explorer job, `is_live` incl. the 60 s failure grace and a backwards clock, the message label); `sse::progress` +8 (TransferDetail on the event/snapshot, `SpeedTracker`: two samples before a rate, the window slope not the run average, pruning, a backwards counter, ETA rounding, the wire field names); `handlers::activity` +2 (the structured detail and the aria2 counters carried as fields)** ; `browser.rs` (**45** — **+2026-10-29 (instant copy): +19 in the new `browser::reflink` submodule — the cascade's method/byte contract (`clone` when the filesystem shares extents, `kernel_copy` when `FICLONE` is refused and the range copy fills the placeholder, `stream` under `mode=off`, an empty file, a missing source as a real error, "never claims a clone it did not get"), `CopyMethod` slugs/ranks/instantness, `CopyStats` (one counter per rank, `slowest` = the fold, `all_instant` only when every file was shared), `summarize_methods`, `InstantCopyMode` (lenient stored parse vs strict request parse), `is_unsupported_errno` (EXDEV/EOPNOTSUPP/ENOTTY/EINVAL/EBADF fall back, EIO/ENOSPC/EPERM do not), `/proc/self/mounts` parsing (octal escapes, longest-prefix mount, a sibling path not confused, btrfs yes / xfs only with `reflink=1` / ZFS never claimed), and `describe_mounts` (dedupe, `/` fallback)** ; 26 — **+2026-10-29 (trash retention): +9 `trash_policy_tests` — default 30 d / 10 GiB bounds, "keep everything" policy, age-only, size-cap oldest-first, age+size combined without double-purging, an unparsable `DeletionDate` never aged out, recursive `path_size`, `purge_trash` reports purged/bytes, missing-root no-op** ; 17 — +6 collision-strategy on 2026-09-07, +1 on 2026-09-13: `copy_dir_all_tracked` records only the created paths ; **+4 on 2026-10-13 (b): `fuzzy_score` — exact/prefix, non-contiguous subsequence, rejection of non-subsequence/empty, contiguous-and-short preference** ; **+2026-10-24 (ROADMAP X.2): 16 → 17 — `test_every_subprocess_carries_a_deadline`, the source guard that keeps `zip`/`tar`/`df` under `run_timed`**), `config/explorer_config.rs` (**9** — +5 on 2026-10-24 (ROADMAP 10.5): a new file comes back under its resolved parent with no `..`; a symlink leaf pointing outside the roots is refused; one inside resolves to its target; a `.`/`..` leaf is refused; the relative branch), `handlers/explorer.rs` (**52** — **+2 on 2026-10-30 (admin-delete scratch exemption): `is_under_temp_root` matches the OS scratch areas and never a string sibling like `/tmpfoo`, and it rejects `/etc`, `/`, `/usr/local` and `/home`** ; 50 — **+4 on 2026-10-29 (instant copy): the tracked-job summary says "instantly" only when the slowest method of the whole tree is instant, the same summary says "N cloned, M copied" for a mixed tree and never the word "instant", a kernel copy is never called instant, and the request/stored parsers differ (strict rejects `on`-style typos while a stored row falls back to the default)** ; 46 — +8 on 2026-10-24 (ROADMAP 10.5/10.6): the admin branch normalises but does not confine, unusable shapes, an existing symlink resolves, a chunk from a 3-frame stream, an over-cap chunk rolls the file back, an empty stream writes nothing, the route cap stays below the router limit; 38 — +5 on 2026-09-07: `kind_for_root`, `resolve_dest`; +5 on 2026-09-13: `parse_range` — absent/suffix/clamping/416/malformed ; **+2026-10-13 (b): `smbtree` + Avahi/mDNS parsers and the `ext4`/`exfat` allow-list** ; **+16 on 2026-10-22 (ROADMAP 10): member-name/link-target validators, tar/zip/7z listing parses, the staging-confinement walker and the staged merge/replace semantics — 17 → 33** ; **+2026-10-24 (ROADMAP X.1/X.2): 34 → 38 — `path_exists`/`path_is_file`/`path_is_dir` on a file, a directory and a missing path, the module deadline ordering, and two source guards: no blocking token inside an `async fn` outside a `spawn_blocking` closure, and no `.output()`/`.spawn()`/`.status()` in production code**), `media_ticket.rs` (5, 2026-09-13), `sse/file_watcher.rs` (**4** — 2026-10-22 (ROADMAP 10): the watch ticket is bound to (user, path) incl. prefix-sibling refusal, redeemable after refused attempts, single-use, unknown/empty), `trash.rs` (**6**, 2026-10-29 — `root_for` prefers the containing allowed root / first match / falls back outside every root; `roots_from` covers roots + the app fallback for admins, adds SMB share recycle folders for everyone, dedups a share under a root), `trash_scheduler.rs` (**4**, 2026-10-29 — a 0/0 policy skips with no baseline, the first run records a baseline, due exactly at the cadence boundary, a size-only policy still runs) | [x] `explorer_integration.rs` (**98** — **+16 on 2026-10-31 (QW-13): checksum inline digest against the known SHA-256 of `abc`, every algorithm (SHA-512 length, BLAKE3 vector), `md5` refused with a `400` naming it, `compare_with` match and mismatch, an `async` job whose digest is read back from `/checksum/result`, an outside-root subject **and** comparison path refused, no session = `401`; a copy recorded in the history with bytes/method/status, the owner-or-admin scoping (a member never sees another user's row, an admin sees every row), an unknown filter refused, `has_more` paging; the active transfers endpoint (`401` without a session, an open upload listed with its declared size and badge then gone after the tray cancels it); and the HV-UX-03 durability path — a session row inserted straight into the database (what a restart leaves behind) accepts the next chunk at the stored offset and completes, an orphan row whose temp file is gone is swept instead of answered, and the per-user cap counts durable rows** ; 82 — **+1 on 2026-10-30 (admin-delete scratch exemption): `test_admin_delete_allows_scratch_outside_roots_only` — a `mkdir`→`delete` round-trip outside the PathGuard root now succeeds, a non-scratch path outside the roots stays refused, and the browsing-root refusal still wins** ; 81 — **+6 on 2026-10-29 (instant copy): a single-file copy reports its strategy and an `instant` flag consistent with it (`instant` iff `clone`/`kernel_clone`), the reply's byte count matches, `mode=off` forces `stream`, a directory copy still returns a tracked job under both modes, `GET` is readable by any signed-in user while `PUT` is admin-only, an unknown mode is refused with nothing stored, and a same-filesystem move reports `method: null` because an atomic rename copied nothing** ; **75** — **+5 on 2026-10-29 (trash retention): policy defaults readable by any signed-in user, `PUT` refused for a non-admin, an admin saves and reads it back (`0` preserved as "off"), `purge-now` refused for a non-admin, and `purge-now` is a clean no-op under a 0/0 policy** ; **70** — **+5 on 2026-10-24 (ROADMAP 10.5/10.6): a streamed multi-frame chunk lands byte for byte and completes, a chunk past the declared size is `413` without moving the offset (and the session still completes with the accepted bytes), an admin `..` path lists the resolved directory, an admin path outside the added roots still works, a non-admin traversal is still `400`** ; 65 — **+1 on 2026-10-24 (ROADMAP X.1): `test_disk_scan_does_not_block_the_runtime` — a shim `lsblk` sleeping 2 s on `PATH`, and a second request that must be served before the scan returns on the single-threaded test runtime** ; **+6 on 2026-10-13 (b): `/explorer/recents` list/touch/clear, `/explorer/history`, `/explorer/network-discovery`, `/explorer/format` (auth split + `fstype` allow-list + non-available device refused)**. **+9 on 2026-10-22 (ROADMAP 10): traversal tar + traversal zip + symlink zip refused with nothing extracted (evil archives crafted with `python3` — GNU tar sanitises member names on the way in), `.gz` streaming round-trip, admin delete confined (`refuses the root`, `inside-root` still ok), watch ticket bound/single-use and streaming, upload ownership end-to-end, per-user cap, compress dash-name regression**; +11 on 2026-09-07 — `/roots` (roots + `kind`), `/save` (write/overwrite/outside-root/folder/giant payload), `/eject` (admin only + refusal of a non-mount-point), `/snapshots` (empty outside ZFS), favorites (add/list/remove), `?strategy=keep_both\|skip` on `/copy`) ; **+9 on 2026-09-13 — transfers v2: `Range` (206/416/suffix), `X-Content-Sha256` digest, download ticket (single-use, without session), thumbnail ticket (multi-use, without session), resumable upload (chunks round-trip → complete, offset mismatch 409, abort without debris), outside-root rename refused, admin-only delete — **49/49 green 2026-09-13** ; **+2026-09-12: `share_links_integration.rs` (12 — auth matrix, create/list, refusal of folder/missing file/invalid mode, non-admin `PathGuard` guard, public download + unknown/revoked/expired token, `signed_in` without/with session, owner-only + admin revoke, `/share/{token}` reachable via the real `create_app` router)** ; **+2026-09-13: `share_links_integration.rs` (12) — `Range` on `/share/{token}`** | [x] `explorer.spec.ts` (**78 runtime tests / 76 `test(` at the grep** — **+2026-10-31 (QW-13): section 18, 7 tests — 18.1 the tray opens from the toolbar with three zones, `aria-pressed` state and the empty state, 18.2 an open chunked upload is listed with `1 KB / 2 KB` and a cancel that removes it server-side with no partial file left, 18.3 a finished copy appears in the history section with its destination and method, 18.4 the checksum endpoint returns SHA-256("abc"), compares two files (match and mismatch), refuses MD5, 18.5 a non-admin is refused on a private path and an anonymous call is `401`, 18.6 keyboard open (Enter) + Space on a zone + Escape closing with focus restored to the toolbar button, 18.7 no horizontal overflow at 360/768/1920 px. **Written, not run yet** — 71 runtime tests / 69 `test(` at the grep** — the two extra come from a responsive loop; `npx playwright test tests/e2e/explorer/explorer.spec.ts --list --project=firefox` = 71 on 2026-10-29. **+2026-10-29 (instant copy): section 17, 5 tests — 17.1 a copy reports its strategy and the byte length is exact, and `instant` matches both the slug and the server's own mount evidence (never a hardcoded clone claim, so the test holds on ext4 and on Btrfs), 17.2 `mode=off` forces `stream` and restores the policy afterwards, 17.3 the admin toolbar opens the policy modal, both radios and the mounts list render, and each choice is persisted server-side, 17.4 an unknown mode is refused with nothing stored and a non-admin `PUT` is `403`, 17.5 a same-filesystem move reports `method: null`. The section runs against a live seeded server and was **run live on 2026-10-30: 17.1–17.5 all pass** (Firefox, scratch instance `:8099`); that run found and fixed the policy modal's design-system leak (global `input` styles sizing `<input type="radio">` to full width → 1246 px dialog with the option text one word per line) and the `openWork` stale-favorite accumulation, both recorded in the QW-03 section below** ; **62 `test(` before — **+2026-10-29 (trash retention): 4.5 the admin-only retention panel (collapsed → expand, 4 age options, reads the real policy), 10.1 now pins `/explorer/trash/policy` on the no-token matrix, 10.7 a non-admin reads the policy (200) but its `PUT` is refused — **all three green live 2026-10-30** (Firefox, scratch `:8099`)** ; **64 at `--list` on 2026-09-24** — +8.5 "Space activates a focused folder row, not just Enter" (W18), written but **not executed**) — **49/49 green 2026-09-07** ; **+2026-09-12: Appliance design migration — `PageLayout` (locations sidebar + breadcrumb + list/grid view + trash), selectors `data-testid="explorer-*"`, zero scoped CSS** + **section 15 "Explorer extras" added on 2026-09-08 (9 tests, 3 isolated runs `-g "15"` green on a fresh server)**: inline editor (the modal exposes Edit → pre-filled textarea), `POST /api/explorer/save` (writes / overwrites / rejects > 5 MiB), favorites (toolbar star ⇄ sidebar ⇄ server, opening from a new window), collision strategies `skip`/`keep_both`/`overwrite` on `/copy` and `/move` (via in-page `fetch`), Snapshots menu ("no snapshot" outside ZFS + endpoint), `/eject` (admin only + refusal of a non-mount-point), "Share this folder" hand-off → the Shares app opens with the pre-filled form. `apiDownloadLen` goes to `cache: 'no-store'` (the only change of a shared helper). **+2026-09-12: section 16 — link sharing (create → anonymous public GET → revoke → 401/400), `signed_in` link (anonymous 401 / session header 200), editor close guard (unsaved changes → confirm "Discard/Save")**. **+2026-09-13: standardization — sidebar refactored onto `WindowSidebarItem`/`WindowSidebarSection` (`.disk-btn`/`.disk-name` kept as test hooks), **icon-rail collapse** (`sidebarCollapseable` + `bind:sidebarCollapsed` → 64px rail instead of disappearing), status footer via `WindowFooter mode="status"` (folder summary + total size + live selection) + **drop of an OS file onto a sidebar destination (root/favorite/mounted disk → upload in place, `is-drop-active` ring)** ; tests **1.6** (collapse ⇄ expanded: `hv-winsidebar--collapsed` class, hidden labels, tooltips, width) and **1.7** (status footer + selection counter) added** ; **the same status footer (Puter `explorer-footer` pattern) is generalized to the data screens: `Syncthing` (folders/devices/connected), `Storage` (disks/virtual/partitions/removable), `Shares` (accounts per protocol), `Downloader` (active/completed), `Network` (interfaces/links/IP)**.** **+2026-09-13 (transfers v2, to re-run live)** : download via ticket → anchor (no more `fetch`→`blob`), thumbnails authenticated by directory ticket (no more `?token=`), resumable chunked upload. **Not covered**: navigation *inside* a real ZFS snapshot (host required). **Pre-existing flakiness/stale locators**: `navigateTo`/`currentPath` rely on `.bc-item` (class absent from the current breadcrumb → intermittent sections 2/3/6/8/10/11) and 1.3/1.4 use obsolete selectors (nested `[data-testid="explorer-page"]`, `…"]2`). Re-run on a fresh server or with `-g`. **+2026-10-30: the whole file is green — 71/71 on Firefox against a live seeded server — and two of the reds were fixed at the cause: 13.1 by the admin-delete scratch exemption (see the QW-03 section), and 1.6, which sampled `boundingBox()` on the first frame of a 180 ms `transition: width` and intermittently read 72 px (both width assertions now `expect.poll`).** |
| 17 | Shares/Protocols | [x] 2026-07-13 (+07-18 real SFTP ; **+2026-10-21 production-ready pass: config-injection validators on every share field (name/comment/path/NFS options/SMB global/FTP banner), `/etc/exports` rewritten through one parser+generator (durable `# homevault-nfs-disabled:` toggle, line-exact delete/update, 400 on ambiguous basenames), all service calls on `spawn_blocking` with 30 s/120 s timeouts (the outer timeout stops waiting but does not kill the child — the child itself is bounded since ROADMAP X.2, 2026-10-24); every mutation now reports a refused call through `notifyError` (ROADMAP X.3, 2026-10-24)** ; **+2026-09-07 end-to-end real test: real SMB share create/toggle/update/delete + service start/stop ; SFTP key → Linux account + group, `setfacl` ACL grant on the root dir, `rebuild_jails` → bind-mount + `/etc/fstab`, revoke, `purge_user`. 5 bugs fixed — SMB toggle no-op on the last section of `smb.conf`, writing `/etc/homevault/*.json` without `sudo`, all `setfacl` without `sudo`, revoke that left the default ACL, `set key` that did `useradd` for a nonexistent name**) | [x] `permissions.rs` (10), `shares.rs` (**41** — **+4 on 2026-10-29 (SMB recycle): the block targets the Explorer trash folder, enabling twice is idempotent, disabling removes the block and keeps every directive, a hand-added `veto files` survives the round-trip** ; +22 on 2026-10-21: the six validators' accept/reject matrices, `/etc/exports` line + disabled-tag round-trips, the SMB section shape; **+2026-10-24: +5 deadlines (ROADMAP X.2)** — source guard, kill path, exit-as-data, missing tool, deadline ordering), `sftp.rs` (**9** — +3 on 2026-10-21: `/etc/fstab` column/line injection, path-fragment usernames) | [x] `shares_integration.rs` (**28** — **+2 on 2026-10-29 (SMB recycle): the recycle map is a name→bool object, a forged `..` share name is a clean 4xx before any write** ; +6 on 2026-10-21: name/comment/path/NFS-option injection, multiline SMB global config, FTP banner + port shape, all 400 before any `sudo`) | [x] `shares.spec.ts` (53 `test(` / 55 runtime ; **+2026-10-29 (SMB recycle): 3.8 a forged `..` share name is refused (400, not 500), 4.8 the SMB tab shows one recycle row per SMB share — written, not re-run live this pass** ; **+2026-10-24: 15.1/15.2 — a mocked refused share delete / SMB config save shows the server reason in a `[data-sonner-toast][data-type="error"]` (ROADMAP X.3)**) ; **+2026-09-14: standardization of headers — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the recap "%total% shares · %active% active" (`shares.summary`) disappeared with the subtitle — the protocol tabs remain. Number of tests unchanged; spec modified but NOT re-run** ; **+2026-09-12: Appliance design migration — `PageLayout` + protocol tabs, `SectionCard`-like forms, `PathField`/`Toggle`/`Modal` kept, selectors `data-testid="shares-*"` (modal → `[data-slot="dialog-content"]`), zero scoped CSS** |
| 18 | Cloud Sync (rclone) | [x] 2026-07-14 (+09-06 test suites, +10-23 ROADMAP 16) | [x] `handlers/rclone.rs` (**44** — +1 on 2026-10-26: `rclone_command()` passes `--config <rclone_config_path()>` explicitly to subprocesses (`mount`, `sync`, `copy`, `listremotes`) so subprocesses resolve the same configuration file as the backend ; +4 on 2026-10-24 (ROADMAP 16.4): the scheduler loop exists and is spawned at startup, the run core returns a plain `SyncOutcome` (so the scheduler can call it), one job cannot run twice at once (`JobRunGuard`), and a saved job replays as a non-destructive sync ; 39 — `build_rclone_path`, `get_provider_description`, `rclone.conf` parser, `MountRequest` defaults; **+2026-10-12**: 3 on `rclone_config_path()` — honours a non-empty `RCLONE_CONFIG`, ignores an empty one, falls back to the default when none exists — and 1 proving the parser and the handlers resolve the same file) ; **+2026-10-23 (ROADMAP 16), 24 new**: the four input validators — `validate_remote_name` (plain name, empty, `:`/`[`/`]`/`/`/`\\`/space/control/leading-`-` refused, 64-char bound), `validate_provider_slug` (known slugs, `s3\ntoken = attacker` and `[evil]` refused — the INI-injection class), `validate_mount_point` (child of `RCLONE_MOUNT_BASE`, trailing slash normalised, nested folder, base itself / `/mnt` / `/tmp/evil` / `..` / `.` / relative / newline / leading-`-` refused) and `validate_remote_subpath` (empty → `None`, leading-slash and traversal refused) ; `config_has_section` (whole trimmed header only — `[mydrive]` is not `[my]`) and `remove_section` (removes the exact section, keeps the prefixing one and comments, does **not** cut on a bracketed value like `patterns = [a-z]`, handles first/last, reports a missing section) ; `parse_mounts_for` + `decode_mount_escape` (whole-field match, `\040`/`\011` octals) ; `parse_rclone_config_text` is now pure (empty file → `None`) and the parser no longer needs a tempfile ; the deadline ordering (`PROBE < MOUNT_READY < UMOUNT < TRANSFER`, 1 h ≤ transfer ≤ 24 h), `CONFIG_FILE_MODE == 0o600`, and the `RcloneStatus` payload (it carries `mount_base`)) | [x] `rclone_integration.rs` (**29** — +3 on 2026-10-24 (ROADMAP 16.4): create-with-interval schedules the row and reports empty state columns, a run records `last_status: failed` with its reason and rolls `next_run_at` forward (visible on the list), switching to `none` clears the due time but keeps the history, an unusable cadence is refused at create time with nothing written, and the schedule route is admin-only ; 26 — +2026-10-12: `config_endpoints_use_the_rclone_config_override`, `save_config_writes_the_overridden_file_and_keeps_a_backup` and `add_list_delete_remote_round_trip_on_the_overridden_file`: the full admin cycle now runs for real against `RCLONE_CONFIG` pointed at a tempdir, which the hardcoded `/etc/rclone.conf` made impossible ; **+2026-10-23 (ROADMAP 16), 9 new**: `add_remote_writes_the_config_and_backup_0600` (a fresh config is 0600, an existing 0644 one is **repaired**, and the dated backup is 0600 — `mode_of` helper ; the existing save test also asserts both modes), `delete_remote_removes_only_the_exact_section` (`[mydrive]` + `scope = drive` survive deleting `my`, only `my`'s keys go), `add_remote_allows_a_name_that_prefixes_an_existing_section` (and a real duplicate is still refused), `add_remote_refuses_a_provider_that_injects_ini_lines` (four payloads, and the guard runs **before** the file is touched), `mount_start_refuses_a_mount_point_outside_the_rclone_base` (nothing is created outside `/mnt/rclone`), `mount_start_refuses_a_remote_that_is_not_configured` (no directory either — the guard precedes `create_dir_all`), `mount_start_refuses_a_remote_name_that_is_not_a_plain_slug` (`gd:other`, `a/../b`, `-oallow-other`, `has space`), `mount_stop_accepts_the_body_the_ui_sends` (`{ mount_point }` alone is **not** a 422 and the refusal names both `fusermount` and `umount`) and `mount_stop_refuses_a_mount_point_outside_the_base` ; the status test now also asserts the `mount_base` field) | [x] `cloudsync.spec.ts` (**29 `test(` / 33 runtime** — "credentials leak" guard on `GET /config` ; real remote cycle **opt-in** `CLOUDSYNC_E2E_WRITABLE_CONF=1`) ; **+2026-09-12: Appliance design migration — `PageLayout` (3 tabs + sync/config modals), selectors `data-testid="cloudsync-*"`, zero scoped CSS** ; **+2026-09-22: sync jobs persisted server-side (`sync_jobs`, migration 036) — the frontend abandoned localStorage (`api.listSyncJobs/createSyncJob/deleteSyncJob/runSyncJob`)** ; **+2026-10-23 (ROADMAP 16): 5 new — 3.6 mount start refuses `/tmp/hv-e2e-escape`, `/mnt` and `/mnt/rclone` itself, 3.7 add remote refuses an INI-injecting provider (`s3\ntoken = attacker`, `drive\n[evil]…`), 3.8 unmount accepts the `{ mount_point }`-only body (not a 422, error names the unmount), and section 7 "Failure surface" stubs one failing `/mount/start` to assert the page alert shows the server's reason (7.1) and that its icon-only dismiss has an accessible name and clears it (7.2)** |
| 19 | Docker | [x] 2026-07-16 (+ ~8 follow-ups up to 09-04) ; **+2026-10-20 — production-ready pass**: volumes tab + `docker system df` + prune, inspect, pause/unpause, kill (whitelisted signals), rename, in-place limit updates, exec, live logs over a single-use SSE ticket, image builds (path-guarded context), private registries (`--password-stdin`), `daemon.json` editor (validated before write), container update-to-newest-image (pull + recreate with rename rollback), Compose `pull` + `up -d`. Every CLI call is now time-bounded (`run_docker`/`run_command`/`wait_with_timeout`, `DockerError::Timeout`) and every mutation runs on the blocking pool | [x] `docker.rs` (**17** — +8 on 2026-10-20: `PruneTarget` round-trip + unknown-target rejection, `parse_reclaimed` (all three shapes, incl. the builder-prune table), `parse_system_df` (real output + header/junk), `parse_container_detail` (fields + garbage), `run_args_from_inspect` (replays settings, skips the default `bridge` network)), `compose.rs` (6), `app_catalog/*` (22 — mod 7, manifest 8, store 3, updates 4), `app_proxy/*` (28 — +2026-10-23: `app_proxy::origin` **8** on the trusted-host resolver (host normalisation, forged-`Host` fallback, IPv6 authority), `app_auth` 6 → **8** (a session is bound to the app it was minted for; `revoke_app` leaves other apps signed in), `serve` 4 → **5** (the `app_auth_location` builder brackets an IPv6 origin) and `handlers/docker_catalog.rs` **4** on the install-marker contract (round-trip, proxy-less marker, a reported write failure, missing marker); +2026-10-21: cookie 6, `mod` 1) | [x] `app_catalog_integration.rs` (10), `app_proxy_integration.rs` (1 end-to-end flow, +2026-10-21 step 7: with `TLS_ACTIVE` up the gate redirects to `https://…:8443/app-auth?app=…` and both auth-minting redirects carry `no-store`; **+2026-10-23 steps 8–10: a forged `Host` never reaches the `Location`, a session minted for `test:echo` is refused on `test:other`, and stopping `test:echo` leaves `test:other`'s session valid**), `compose_integration.rs` (6), **`docker_integration.rs` (10, 2026-10-20 — auth sweep over every new endpoint (no token / non-admin → 403), log-ticket rejection (unknown ticket, ticket minted for another container), unknown prune target rejected before the daemon is contacted, volume-name and container-name validation, `daemon.json` validation (malformed JSON, array, string, empty) + the read shape on a fresh install, build-context rejected outside the `PathGuard` roots)** | [x] `docker.spec.ts` (**67 `test(` / 69 runtime** — +2026-10-24 (ROADMAP 31.1): §17 "API client consolidation" (3 tests, no docker daemon needed) — 17.1 every `/api/docker` request carries `X-Session-Token`, `X-CSRF-Token` **and** `X-HomeVault-Contract` (the deleted local `authedFetch` sent no contract header), 17.2 no served JS chunk contains a raw `fetch('/api…)`, 17.3 a container action is not answered from the 3 s `GET /docker/containers` cache (the stubbed list differs before/after the action) ; 64 `test(` / 66 runtime, incl. real opt-in cycle §13 on a disposable `hello-world` container, + "Edit" compose button §4.8) ; **+2026-09-12: Appliance design migration — nav rail + metrics + 6 tabs (incl. catalog + compose) + inline modals, selectors `data-testid="docker-*"` (modal → `[data-slot="dialog-content"]`, toggle → `[role="switch"]`), zero scoped CSS** ; **+2026-09-13: nav rail + metrics section refactored onto `WindowSidebarItem`/`WindowSidebarSection` (`role="tab"`, `aria-selected`, roving `tabindex`, `docker-nav-item` kept)** ; **+2026-10-13: catalog phase B (feature parity with the reference design — `implements` alternatives, `[[environment]]` settings, `min_homevault_version`, install-review, update-all queue, per-app logs) + MCP `apps.*` sharing the install core — `docker.spec.ts` 51 → 56 `test(` / 58 runtime (§15: review shape, badge/button states, dialog resolve-and-cancel, update-all + logs gates, bad-alternative rejection), full suite 55 passed / 3 conditional skips / 0 failed (Firefox, scratch `:8099`). Two adjacent fixes in the same run: 4.5 was red on a stale selector (the stores toggle never exposed `aria-expanded`, missing even at HEAD — now a native `hv-btn-secondary` button with `aria-expanded`, same skin) and 5.1 raced the async `compose/available` probe (now waits for its response before reading `disabled`)** ; **+2026-10-20: the screen grows 6 → 7 tabs (new **Volumes**: `docker system df` table + 4 prune buttons + volume list/create/delete), a "more actions" menu per container card (details, pause/resume, kill, rename, limits, exec, update image), a rebuilt logs modal (live SSE tail, tail size 100–5000, timestamps, follow, status pill, download, restart stream) and 8 new modals (volume, inspect, exec, rename, limits, build, registries, `daemon.json`). New §16 "Production surface" (**56 → 64 `test(` / 66 runtime per browser**; 8 tests: disk-usage table + 4 prune buttons, volume modal validation, build/registries modals, `daemon.json` editor, the "more actions" menu + Escape, the live-log SSE modal, a real `exec`). `npm run check` 0 errors / 0 warnings, `npm run build` OK (Docker chunk 126.7 kB). **Re-run 2026-10-20 against a live daemon (scratch server `:8098`, `e2eadmin`): 123 passed / 9 skipped / 0 failed across firefox + chromium.** Three real defects were found and fixed by that run: (a) both card popover menus were clipped away by the card's `overflow-hidden` — they are now `position: fixed` with a viewport-anchored position, so they are visible and clickable on a short card; (b) Escape did not close them while focus sat on the trigger — now a `window` keydown listener; (c) 5.2 compared a container count taken before the list settled (false failure on a loaded machine) — it now asserts the absent name instead. The suite also acknowledges the release notes in `beforeAll` (`POST /api/whats-new/seen`), without which the What's New overlay swallows the first dock click on a fresh server |
| 20 | Syncthing | [x] 2026-07-16 (+09-06 test suites, +10-23 ROADMAP 18.2, +10-24 ROADMAP 18.1) | [x] `handlers/syncthing.rs` (**34** — `extract_api_key`, deserialization of the REST responses + `certName`/`deviceID`/`myID` aliases ; **+2026-10-24 (ROADMAP 18.1), 16 new**: 3 for `validate_folder_id` (accepted ids, each way an id can be unsafe — blank/`.`/`..`/`/`/NUL/length —, the trim), 3 for `validate_folder_path` (inside a managed root; relative/`..`/root/outside-roots/no-roots, the last one refusing **everything**), one each for the label (trim/control characters/128), the mode allowlist (`receiveencrypted` included among the refusals), `normalize_device_id` (pasted with spaces, lower-case, undashed → canonical; wrong length, non-base-32, empty), the name fallback to the first group, the device dedupe/cap, the config duplicate checks, `folder_devices_with_self`, and the two request bodies compared against the daemon's own schema ; **+2026-10-23 (ROADMAP 18.2), 7 new**: the API-key → header matrix — a realistic key, the indented value of a pretty-printed `config.xml` (the newline that used to panic), newline/CR/NUL/DEL rejected as a named error that never echoes the key, non-ASCII rejected, a blank key meaning "no key" (no header, not an error), the `build_headers` matrix, and a source guard that fails if `danger_accept_invalid_certs` comes back on the daemon client) | [x] `syncthing_integration.rs` (**12** — **admin** guard on the **14** endpoints (the four management routes included, in both directions), 422 body, `status` shape ; **+2026-10-24 (ROADMAP 18.1), 4 new**: `POST /folders` refuses eight malformed bodies (blank id, `/` in the id, relative path, `..`, outside the roots, path over the cap, `receiveencrypted`, an unpaired share target) with the field's own reason and **no** `Failed to connect` (so validation provably runs first), `POST /devices` refuses five (empty, wrong length, non-base-32, control characters, and a valid id whose name is bad), both `DELETE`s refuse a `..`/`a%2Fb`/malformed segment after URL decoding, and a body that passes every rule still reaches the transport on a host with no daemon — which is what proves the rules are not refusing valid input ; **+2026-10-23 (ROADMAP 18.2), 3 new**: a `config.xml` under `DATA_DIRECTORY` with a newline in the key makes `/config` (and `/status`, when the binary exists) answer a *named* `success:false` error instead of panicking the task, an indented key is accepted, and a blank key is never reported as a key error) | [x] `syncthing.spec.ts` (**34** runtime / **30** `test(` — "unavailable" state, guards, daemon-off error path ; **+2026-10-24 (ROADMAP 18.1), 7 new in §7 against a mocked daemon** (fixtures for the reads, every write recorded, so no real daemon config is ever mutated): 7.1 the *Add folder* dialog keeps Create disabled until the id **and** the path are filled and sends nothing on typing alone, 7.2 the payload is the trimmed form with the pre-ticked device and the list refreshes under a success banner, 7.3 a refused create stays **inside** the dialog with the typed values intact and no page banner, 7.4 a malformed device ID is explained in the pairing dialog, 7.5 a folder removal names its consequence, sends **nothing** on Cancel, then deletes on confirm, 7.6 the same for a device (files stay on disk), 7.7 both *Add* actions are labelled buttons reachable with Enter and the dialog closes on Escape — **written, not run** (per instruction) — live read **opt-in** `SYNCTHING_E2E=1` ; **+2026-10-23 (ROADMAP 18.2): 4.4 a stubbed unsendable-key status renders `syncthing-error` *and* suppresses the "install Syncthing" panel, 4.5 a stubbed refused scan surfaces the server reason instead of doing nothing**) ; **+2026-09-12: Appliance design migration — `PageLayout` (3 tabs), selectors `data-testid="syncthing-*"`, zero scoped CSS** ; **+2026-09-13: 2-pane master-detail for the Devices/Folders tabs (compact list on the left, detail pane on the right; `syncthing-device-list-row`/`syncthing-folder-list-row`, `ResizeObserver` split < 720px → a single pane) ; the Overview tab remains unchanged ; `syncthing-btn-primary`/`syncthing-empty`/`syncthing-stats` kept (E2E unchanged)** |
| 21 | Downloader | [x] 2026-07-16 (+07-17, +09-06, +10-23 test suites) | [x] `handlers/downloader.rs` (**32** — serde mapping of the real aria2 camelCase format ↔ snake_case JSON returned to the frontend ; request bodies ; `ApiResponse` envelope ; **+2026-10-23, 17 new**: `validate_uri` scheme allowlist (`http`/`https`/`ftp`/`sftp`/`magnet` accepted ; `file://`/`data:`/`javascript:`/`vbscript:`/`smb:`/`ssh:`/bare paths refused), empty/whitespace/control/over-long links, host requirement, magnet `xt=` ; `validate_subfolder` relative/absolute/traversal/backslash/control/oversize matrix ; `validate_output_name` bare-name vs path matrix ; `resolve_download_dir` root confinement incl. a **symlink that leaves the root** and a missing root ; `redact_userinfo` on links carrying `user:pass@` ; the per-process `rpc-secret` shape/uniqueness) | [x] `downloader_integration.rs` (**13** — admin guard on the 10 endpoints, 422 body validation, safe `GET /status` path ; **+2026-10-23, 5 new**: `dir` lives under `DATA_DIRECTORY` (not `/tmp`), `/status` never serialises the RPC secret, a crashed child is reaped and reported `running: false`, and three refusal tests — non-allowlisted scheme, destination outside the root, path-like `out` — each asserting aria2 was **never spawned**) | [x] `downloader.spec.ts` (**46** `test(` / 48 runtime — incl. a **real download** of a file served by the app, through to `complete` on disk, then deletion) ; **+2026-09-12: Appliance design migration — `PageLayout`, selectors `data-testid="downloader-*"`, zero scoped CSS** ; **+2026-09-13: 2-pane master-detail — compact list of the downloads (active + completed) on the left, detail pane of the selected task on the right (re-resolved by gid at each poll to follow live progress ; `ResizeObserver` split < 720px → a single pane) ; `downloader-card`/`downloader-card-name`/`downloader-action-danger`/`downloader-section-title` kept on the rows (E2E 10.x unchanged)** ; **+2026-10-23 (ROADMAP 17): 6 new — 1.7 opening the app no longer starts the daemon (no `/downloader/start` request, the explicit start panel is shown and works), 3.4 a `file://` link is refused with a visible reason and adds no task, 3.5 a `../etc` sub-folder is refused, 5.3 a stubbed failed `add` surfaces the server reason and keeps the form open, 8.4 a task card is selectable with Enter (real `<button>`, not `role="button"`), 10.6 the API refuses `file://` even for an admin and never echoes the secret ; 2.1/4.2/8.1 rewritten for the sub-folder field (`downloader-subfolder`, `downloader-destination`) and the no-nested-controls rule ; the real download of section 13 now lands in `<DATA_DIRECTORY>/downloads/hv-dl-e2e-*` instead of `/tmp`** |
| 22 | Backup (Restic) | [x] 2026-07-14 (+07-17, +09-06 test suites ; **3 bugs found and fixed** incl. the entire page dead in the browser, cf. below) ; **+2026-10-13 (b): backup reminders banner, split-view restore dialog with a debounced pre-flight space check, floating progress island** ; **+2026-10-21: production-ready pass — backup/restore are background jobs with real streaming progress and real cancellation, every argv value is validated, one run per repository, stale jobs reaped at startup, `max_size_gb` actually applied** | [x] `restic_scheduler.rs` (**16** — `compute_next_run` + `normalize_crontab_dow`, **+4 on 2026-10-21: `plan_keep_last_for_size`**), `restic.rs` (**35** — `parse_backup_summary` NDJSON, snapshot deserialization, `restore_size`; **+21 on 2026-10-21: every `validate_*` helper, `parse_check_output`, `parse_backup_line`/`ResticProgress`, `RepoRunLock`, `cancel_run`, `forget` refuses an empty rule set**; **+2026-11-05 (P1-10): +6 — derived mount point confinement, deterministic path-free slug, exact `restic mount` argv, `/proc/mounts` parsing (incl. the real restic `ro,nosuid,nodev` line), only read-only mounts under the root advertised, unmount id traversal refused — 29 → 35**), `handlers/restic.rs` (**+3 on 2026-10-13 (b): `days_since`, `parse_df_available`, and the restore-plan size/space comparison** ; **+3 on 2026-10-21: `validate_retention_limits`**) | [x] `restic_integration.rs` (**26** — **+2026-11-05 (P1-10): +2 — a mount of an unknown repo is a clean 400/503 never a 500, the mounts list is an array, and an id with `..`/`/` is a 400 before any host command — 24 → 26**; **+3 on 2026-10-13 (b): `GET /restic/reminders` (per-repo `last_backup_at`/`days_since`/`stale`, auth-guarded), `GET /restic/restore-plan` (size vs. available, auth-guarded), and the `include_password` guard still holds** ; **+7 on 2026-10-21: repo-create validation before the availability probe, relative source path, busy repository, `forget` without a rule / bad `keep_within`, option injection on `snapshot_id` and the restore target, `read_data` admin, zero + second-default policy, stale-job reaper and `snapshot_id` recorded on completion**; auth split, schedule + retention cycles 100% DB, never `restic`) | [x] `backup.spec.ts` (30 `test(` / 32 runtime — incl. a **real restic backup** in a disposable repository + verification of the cron day of the week ; **+2026-09-22: 14.3 — "Restore" regression audit: the modal acts on the snapshot whose button was clicked (2nd row), never the first anymore — restic API entirely mocked in the browser, runs without restic**) ; **+2026-09-12: Appliance design migration — `PageLayout` (repos / creation / detail, snapshots-jobs-schedules, retention policies), selectors `data-testid="restic-*"`, zero scoped CSS** ; **+2026-09-13: the snapshots/jobs/schedules tabs go from the hand-made inline bar to the single tab row of the `WindowHeader` (`tabs`/`bind:activeTab` on `PageLayout`, counters kept)** ; **+2026-09-13: 2-pane master-detail — repository cards on the left (420px column, stats at the top), repository detail / creation form on the right (`ResizeObserver` split < 720px → a single pane), `restic-repo-name` still rendered on the list pane** ; **+2026-10-13 (b): the restore dialog is now split-view with a pre-flight space check, and the reminders banner + floating progress island are new. Spec not re-run** ; **+2026-11-05 (P1-10): +2 — 2.3 the selected repo shows Browse, the mounted path or the FUSE-missing note; 3.7 mounting an unknown repo is 400/503 and an unsafe unmount id is 400 — 30 `test(` / 32 runtime** |
| 23 | Rescue | [x] 2026-07-17 (+07-17, +09-05 ; **+2026-09-07: 1st real run on 4 loop devices + disposable RAID1 — ddrescue clone / testdisk analyze / photorec / mdadm diagnose+repair end to end ; 6 frozen bugs fixed — sudo-wrap ddrescue/testdisk/photorec, `lsblk -P -b` (model/transport/removable/size), `/dev/md127:` header in `mdadm_diagnose`, "clean, degraded" health→healthy, `parse_testdisk_output` offsets, `testdisk_rebuild` no-op + anti-hang**) | [x] `rescue/validator.rs` (8 → **25**) + `rescue/models.rs` (**7** — option matrices) + `rescue/service.rs` (8 — `parse_ddrescue_mapfile` ×5, `parse_testdisk_output` ×3) + `rescue/jobs.rs` (**4** — registry + cancel flag) + `sse::progress` cancel hooks (**+6**) | [x] `rescue_integration.rs` (28 → **38**) + `progress_integration.rs` (7 → **9**) | [x] `rescue.spec.ts` (57 `test(` / 59 runtime) ; **+2026-09-14: standardization of headers + moving of the availability badges** — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)` ; 1.3 is REWRITTEN: the band of 4 badges that lived in the header disappeared (decorative element), availability is now on the tool's card (`SectionCard`'s `actions`), one per tab — the `data-testid="rescue-tool-badge"` is kept but moved to a wrapping `<span>`, without which `Badge` could not guarantee its transfer via `$$restProps`. The test therefore verifies: no badge in `.hv-winhdr`, exactly one on the active tab. Number of tests unchanged; spec modified but NOT re-run** ; **+2026-09-12: Appliance design migration — `PageLayout` (4 tabs) + `SectionCard`, `PathField` kept, selectors `data-testid="rescue-*"`, zero scoped CSS ; the legacy components `Card`/`CardHeader`/`CardContent`/`ProgressBar`/`Divider` (last consumer) removed from `theme/`** |
| 24 | AI Assistant (Chat) | [x] 2026-07-13 (+09-02, +09-06 test suites ; **3 "working chat" fixes** ; **verified live with a real Ollama 0.33.3 + gemma3:1b** ; **+2026-10-22 ROADMAP-4: the knowledge base gained its surface + integrity contract, see the Summary addendum of that date**) | [x] `handlers/chat.rs` (17 — localized `system_prompt`, `session_error_message`, `reset_ai_model`, `recommend_models` (extracted, pure), Ollama deserialization ; **+2026-10-19: `test_recommend_models_prefers_bigger_models_on_a_beefy_host` gained the `#[test]` attribute it was missing — it compiled but never ran — 13 → 14**) + `mcp.rs` (7 — scopes, permission parsing, file guard incl. denylist, token mint/verify, tool filtering) + **`rag.rs` 10 → 15 and `rag_indexers.rs` 3 → 5 on 2026-10-22 (prune/clear semantics; embedding fail-loud leaves nothing behind; `index_all` reconciles and fails loudly)** | [x] `chat_integration.rs` (9 — session guard on the 7 endpoints, admin guard on pull/model-set, model-set cycle) + **`rag_integration.rs` (8, new 2026-10-22 — `/api/rag/*` auth boundaries, search error-vs-empty, index → search → reconcile → remove through the `EmbeddingProvider` seam)** ; `dependencies.rs` (+1 — `install.sh` does install Ollama, **strengthened on 2026-09-23**: the test also requires `systemctl is-active --quiet ollama` and the probe `http://127.0.0.1:11434/`, the installer's final check controlling binary + active unit + API as the Ollama Linux doc requires) ; **+2026-10-13: `mcp_integration.rs` (11 — `/api/mcp/*` admin matrix, token lifecycle incl. disabled-server 401, permission round-trip + widening rejections, RPC auth/protocol errors, granted-tools calls incl. file-scope allow/deny, catalog install-review/update-all/logs, bad-alternative install rejection)** | [x] `chat.spec.ts` (20 `test(` / 22 runtime — Ollama off → clean fallback + "AI offline" badge ; **+2026-09-12: Appliance design migration — side history + message thread in Tailwind-on-tokens, selectors `data-testid="chat-*"`, zero scoped CSS** ; **suite `CHAT_E2E=1` 22/22 with real Ollama**, incl. §13.1 real streamed conversation) ; **+2026-10-13: §15 External agents (6 tests) was written here and **moved out on 2026-10-19** — the MCP gateway panel left the Chat app for Settings → AI Assistant, so those tests now live in `tests/e2e/settings/mcp.spec.ts` (7 tests). `chat.spec.ts` is back to **20 `test(`** ; the run that wrote §15 was 27 passed / 1 opt-in skip (Firefox, scratch `:8099`). One real UI bug found by 15.4 stays fixed: `api.mcpStatus()` used `{ttl: 0}`, which is falsy and fell back to the 5 s default TTL — the token list stayed stale after create/revoke; now `{cache: false}`** |
| 25 | Widgets | [x] 2026-07-16 (+07-18, 07-19) | n/a — DB-only handlers (`monitor_layout`, `active-sessions-count`) | [x] `widgets_integration.rs` (**14**, +2026-09-07 — `GET/POST /api/monitor-layout`: session guard, `null` before saving, blob round-trip, upsert = 1 row, partitioning by account ; `GET /api/security/active-sessions-count`: admin guard, deduplication per user + expired sessions ignored ; **+2026-10-23: 9 → 14, ROADMAP 15.1** — the blob is validated before it is stored: an oversized (> 16 KiB), empty, unparsable, non-object (`[]`, `42`, `null`, a quoted string, `true`), too-deeply-nested or over-long-key/string payload is a `400` and leaves the stored layout untouched, while a blob of exactly 16 KiB is accepted and round-trips byte for byte; `has_server_update` is asserted absent from `/widgets/update-info`) ; **+2026-09-14: 6 → 8 — `refreshMs` lives in the same `monitor-layout` blob (no migration, the handler stores the JSON verbatim) ; a legacy blob without `refreshMs` round-trips to the byte without the server inventing one, and a new value persists while leaving its neighboring keys intact** ; **+2026-09-22: 8 → 9 — `GET /api/widgets/update-info`: session guard + without `UPDATE_CHECK_URL` the widget does not fabricate an update (`has_app_update=false`), version comparison unit-tested (`is_newer_version`)** | [x] `widgets.spec.ts` (**23 `test(` / 25 runtime**, 9 categories — **+2026-10-23 (ROADMAP 15): 3.6** the stored layout carries a real `savedAt` stamp (last write wins across origins) alongside the coerced `refreshMs`, **8.4** a malformed blob (an array, invalid JSON, empty, 5 levels deep, > 16 KiB) is `400` and the stored layout is unchanged; previously 21 `test(` / 23 runtime, 9 categories — **23/23 green 2026-09-12** ; snapshot+restore of the layout ; toggle of a tile → desktop monitor panel appears/disappears, footer, Reset, `POST /monitor-layout` persistence) ; **+2026-09-12: Appliance design migration — `PageLayout` + `WindowFooter`, Tailwind-on-tokens tiles, zero scoped CSS, selectors `data-testid="widgets-*"`** ; **+2026-09-12: 2.2 consistent geometry of the 11 panels (card radius 12px, header 48px, close button 44px not clipped, widths 280/300/320 via `--hv-monitor-w*`)** ; **+2026-09-15: panel header = app bar — `WidgetHeader.svelte` goes from "blue icon + label + action/close" to the `WindowHeader` anatomy: 44px white bar (`--hv-widget-header-h` 48 → 44), 28px accent badge (`--hv-winhdr-*`, color per monitor = that of `WIDGET_DEFS`, white glyph) + 14px/600 title, **without a close/action button** (visibility is managed in Settings > Widgets) ; the 11 `desktop.close_*_monitor` i18n keys removed ×4 locales ; 2.2 rewritten (header 44, badge 28 centered, no more close button) — 23/23 green** |
| 26 | Settings (core) | [x] 2026-07-16 (+ ~5 follow-ups up to 09-04 ; **+2026-09-07 real Firewall test: real `ufw enable`/`disable` cycle (port 22 protected), `{confirmed}` lock, 403 on mandatory rule, rule add/delete. 1 bug fixed — `firewall.rs::get_status()` read the English string `Status: active` → on a non-English locale (`fr_BE` → `État : actif`) the firewall always displayed "disabled"** ; **+2026-10-21: Firewall/ufw production-ready pass** — argv validation for action/protocol/port/source/comment, `spec`-based delete, 404 on a rule that is not live, `enable_verified` (re-reads the live state, rolls back when a mandatory rule is missing), a confirmation gate on disable, `detect_ssh_ports()` reading the `sshd_config.d` drop-ins, `tokio::process` + 30 s timeout, and the default policy + change history surfaced in the API and the card) | [x] security cards (`firewall.rs` **31** — 2026-10-21, was 9, `antivirus.rs` 8, `ransomware_watcher.rs` 7, `ransomware.rs` 3) ; `snmp.rs` (**5** — +2026-09-15: parser rewritten on the **numeric** OIDs (`snmpwalk -On`) ; the old one matched the symbolic names `IF-MIB::ifDescr` which do not exist without the MIB files → interfaces/storage always empty ; new tests `parse_interface_stats`/`parse_storage_stats`/`split_oid_line`/`normalize_mac` on real numeric outputs), `ups.rs` (**7** — +5 on 2026-09-15: `parse_ups_conf`/`upsert_ups_section`/`remove_ups_section`/`validate_ups_entry` for the device sections of `ups.conf`), `quota.rs` (20 — `bytes_to_quota_kb`/`format_size_kb` plus the 2026-10-22 backend/read-back/sudo suite, see row 5 ; the group quota tests removed with the feature) ; **core (2026-09-08): `handlers/settings.rs` (**17** — `parse_version`/`is_newer_version`, `sanitize_hostname`, `UpdateFrequency` default = `Manual` (G2), appearance defaults, `normalize_accent` which rewrites the legacy accent `#4a90d9` → `#1E62AE` §1.4.3 ; **+2026-10-22 (ROADMAP 13.1): `is_check_due` — the scheduled-check gate, 2 tests: no interval (`Manual`/`Disabled`/absent) never fires whatever the timestamp, and an opted-in schedule fires exactly at its interval, not before, and not on a backwards clock ; **+2026-10-23 (timezone picker): `timezone_names` — the list offers the IANA database (`Europe/Lisbon`, `Africa/Casablanca`, `America/Sao_Paulo` all present, >300 entries), is sorted and unique, every name parses as a `chrono_tz::Tz`, and the region-less aliases (`CET`, `GB-Eire`) stay out of the picker while remaining accepted on write**) + `settings_persistence.rs` (15 — `export()` masks `smtp_password`, `import()` round-trip, re-import of a masked export does not lose the password)** ; **+2026-10-24 (ROADMAP 25): `src/autostart.rs` 0 → **9** unit** — `args_for` builds exactly `systemctl enable|disable homevault` (the sudous-scoped argv) and `is_enabled_args` exactly `is-enabled homevault`; the unit name is the module constant and no request field can reach it; `enabled_from_output` maps exit 0 → `true`, exit 1 + clean stderr → `false`, exit 1 + stderr → `Err(reason)` (this is what catches 25.2's `unwrap_or(false)` collapse); `set_enabled_outcome` returns the stderr tail verbatim and explains a silent non-zero exit; both deadlines are bounded and ordered | [x] `security_integration.rs` (**26** — 2026-10-21, was 18: both confirmation gates, the pre-ufw input rejections, the 404/403 delete paths, the history envelope) ; `ups_integration.rs` (**5**, 2026-09-15 — `/ups/config` GET/POST/DELETE: without token → 401, invalid token → 401, non-admin → 403, admin GET → 200 ; **+2026-10-23** a write with a bad name / driver / port is a `400` before `sudo` is reached, and a successful write is covered live only) ; **core: `settings_core_integration.rs` (**33**, +2026-10-24 (ROADMAP 29.1): the aggregate carries the live `mdns_enabled` flag; 32, 2026-10-24, ROADMAP 26 — **was 19**, 2026-09-08 — admin split on reads+writes (`/settings/appearance` removed from the admin read matrix: it is a session read since 26.1), `/settings/language` any session, `/settings/public` without auth, appearance round-trip (wallpaper/accent), `GET /settings/export` masks `smtp_password`, export→import, `reset` restores the defaults (incl. G2), notification threshold clamp ; **+2026-10-22 (ROADMAP 13.1), 5 tests against a local listener substituted through `HOMEVAULT_VERSION_CHECK_URL`: the default (`Manual`) and `Disabled` frequencies make **zero** outbound attempts (payload has no `latest_version`, `LAST_UPDATE_CHECK` untouched, `update_check_attempts()` unchanged), `POST /settings/version/check` forces exactly one call and the next GET serves that cache with no second call, a non-admin's forced check is refused **before** any call, and a `daily` schedule fires once, not twice, then again after the interval ; **+2026-10-23: `GET /settings/timezones` — a read that needs a session but not admin (400 without a token, 200 for a regular user), >300 sorted unique names, `Europe/Lisbon` offered, every name parses** ; **+2026-10-24 (ROADMAP 25), 24 → **28**: a `PATH` shim stands in for `systemctl`/`sudo`, so no host command runs — a disabled unit is `200 {enabled:false}`, a probe that writes stderr (container / no systemd) is `500` with the reason, a refused `sudo` is `500` whose body carries `polkit refused: no authorization`, and a successful toggle is `200 {enabled:true}`; `/settings/autostart` stays admin-only on both verbs (added to the no-token / bogus-token / non-admin matrices)** ) | [x] `settings.spec.ts` (**112 `test(` / 115 runtime** — +2026-10-24 (ROADMAP 29.1): 3.7 the mDNS switch is staged and PUT inside the General save, and 3.4 re-targeted from `.first()` to `[data-testid="settings-autostart-row"]` because the General card now carries two switches; 111 `test(` / 114 runtime — 2026-10-24, ROADMAP 26: +4.8 a **non-admin** desktop paints the server accent `#e2725b` on `:root` although `localStorage` says `hv-02` and the server wallpaper replaces it, +4.9 a member reads 200 with exactly two keys / `PUT` → 400 / the grid renders read-only with the member line, +4.10 no `#density`/`#theme` control, no `data-theme`/`data-density` on `<html>`, and the Save PUT body is exactly `['accent_color','wallpaper']`, +4.11 a member's Settings load does not depend on the admin aggregate — **written, not run in this pass** ; 94 `test(` / 97 runtime — 77/77 green on 2026-09-09 ; **+2026-09-12: Appliance design migration — `PageLayout` (category rail) + auto-save cards in Tailwind-on-tokens, embedded security/TLS/monitoring cards kept, selectors `data-testid="settings-*"` (16.5 rewritten for the standard PageLayout anatomy), zero scoped CSS** (full run) ; new 2.5 + 12.2 and sections 1/2/12/16 re-verified green against the rebuilt binary ; **(a) sidebar grouped 12 → 7 categories** (email→Notifications, weather→Appearance, updates/info/data/privacy→System) ; `openCategory()` selects by label, not by index ; **(b) removal of the "Save" button — auto-save per group (700 ms debounce)**: section 15 "Save All" → "Auto-save" (15.1 a single PUT of the modified group, 15.2 persistence after reopening, 15.3 header status, 15.4 no change = no PUT), 21.3 → coalescence of rapid edits, 1.2/1.3/4.6/9.4 adapted ; test 11.2 removed — `.settings-privacy-link` = dead CSS in the frozen `Settings.svelte` ; 4.3 `loading=lazy`→`decoding=async`, 4.2 16→15 thumbnails ; **4.4/4.5/18.3: wallpaper thumbnail reactivity bug FIXED on 2026-09-09** — `isWallpaperSelected()` (helper reading `$wallpaperStore`) only refreshed `class:is-selected`, not `aria-checked` nor the ✓ badge ; replaced by a reactive `$: selectedWpUrl` — the 3 update on click) ; **+2026-09-09: test 2.5 added (78) — no card truncates its content ; layout bug FIXED — `.set-content` is a flex column and each card has `overflow:hidden`, which brings its minimum flex size to 0 → flexbox crushed the tall cards and their content was cut ; `.set-content > :global(.settings-card) { flex-shrink: 0 }`** ; **+2026-09-09: test 12.2 added (79) — in Security, no more manual path entry: each folder field is a read-only display + "Select a folder" button that opens `FloatingPathPicker` (floating movable panel, non-modal, `use:portal` to `<body>`, closes on Escape)** ; **+2026-09-09: wallpaper display bug FIXED — `.settings-wallpaper-thumb` did not have `height:auto`, the `<img height="200">` attribute fixed the height → thumbnails in distorted vertical bands on a narrow grid ; **+ box-shadow removed from the thumbnails** (the `--hv-shell-shadow-card` bled ~24px and made the rows look stuck/overlapping ; selection = `inset` ring that does not overflow) ; 2-column grid below 560px, gap 12px everywhere ; test 4.7 extended (column gap == row gap, no external shadow) ; **8th category "Widgets"** added, the Weather widget moved from `appearance` to `widgets` (`settings.widgets_title`/`_subtitle` ×4 locales) ; `settings.spec.ts` CATEGORIES/GROUP_OF/GROUP_LABEL + test 1.4 "8 entries" adapted** ; **+2026-09-09: "Disk quotas" card (per group) + the entire `/api/quotas` backend REMOVED** — `QuotaCard.svelte`, `handlers/quotas.rs`, `storage/quota_repo.rs`, `tests/quotas_integration.rs`, the group methods of `QuotaService`, `create_group_with_quota`/`delete_group_cascade` (0 caller) and the i18n keys `settings.disk_quota_*`/`quota_*` ; category renamed "Monitoring and storage" → "Monitoring" ; test 13.1 asserts the absence of the quota card ; quotas are managed from Users/Groups**), `security.spec.ts` (**41** `test(` / 44 runtime — 2026-10-21, was 30, incl. §5b which mocks the firewall endpoints with `page.route`) ; **+2026-10-19: `mcp.spec.ts` (7 `test(` / 7 runtime — the external-agent gateway card of the `ai` category: card present in AI Assistant, endpoint + kill-switch + permission fields + token form, kill-switch flip + restore, token create-list-revoke, non-admin invisibility + `POST /mcp` 401, secret hygiene, and 15.7 pinning the Chat app free of the old panel — **7/7 green on 2026-10-19** (Firefox, Vite dev server `:5199` → backend `:8080`, `API_URL` set so the anonymous `POST /mcp` probe bypasses Vite's SPA fallback); the chat suite was re-run in the same pass: **21 passed / 1 opt-in skip** after the panel was removed) ; **+2026-09-09: "HTTPS / TLS" card (Security) — built-in opt-in HTTPS (self-signed `rcgen` + provided certificate, HTTP→HTTPS redirect, conditional HSTS). Rust: `src/tls.rs` (7 — effective port, port/path validation, self-signed generation + introspection, idempotency) + `src/handlers/tls.rs` (1) + `tests/tls_integration.rs` (8 — admin guards 401/403/200, rejection of port==HTTP, manual mode missing paths/`..`, self-signed persists + `requires_restart`, `/regenerate` writes the cert, real PEM pair accepted, no key leak). Live verified: `https://:8443` → 200, HSTS present in HTTPS / absent in HTTP loopback, 301 redirect outside loopback, SAN = localhost/hostname/homevault.local/LAN IP. E2E `settings.spec.ts` 12.3–12.6 added (card loading, field reveal, port==HTTP → Save disabled, no key in URL/localStorage)** ; **+2026-09-13: category rail refactored onto `WindowSidebarItem` (icon-rail collapse `sidebarCollapseable` + `bind:sidebarCollapsed`, `settings-nav-item`/`set-nav-active` kept)** ; **+2026-09-14: complete re-skin on the Figma mockup `Untitled` node `16-6187` — 2-line 208px rail (`WindowSidebarItem variant="rich"`: 28px badge tinted at 9%, flat fill + white glyph + 4×20 bar on the right when active), content header (36px badge + title + subtitle), and shared vocabulary section + card + row (`components/settings/vocabulary.ts`) applied to the **8 categories** ; the 6 embedded cards (TLS, Antivirus, Anti-Ransomware, Firewall, UPS, SNMP) **rebuilt** on this vocabulary with frozen behavior (class/id hooks kept) ; new `theme/Switch` (real `input[type=checkbox]` in `.settings-toggle-label`, 40×22 track, 16px knob) which replaces `Toggle` on the **whole** screen, TLS card included (its 2 toggles were the only 2 callers of `Toggle` in Settings ; `theme/Toggle` remains the `role="switch"` button primitive of the other apps — Shares, Docker, Restic, Storage) — the `.hv-toggle-switch` (44×24, 20px knob, brand blue) that they targeted no longer had a producer, hence the pre-existing failure of 12.3/12.4/12.5, rewritten on `flipSwitch()` ; page-scoped palette `--hv-settings-*` + 8 accents `--hv-settings-cat-*` in `tokens.css` ; **Widgets** category wired to the real thing — the 11 monitors of `WIDGET_DEFS` and a refresh interval persisted in the `monitor-layout` blob (`refreshMs`, **zero migration**: `POST /api/monitor-layout` stores the JSON verbatim) ; `settings.spec.ts` **80 → 95 tests** (11 new in §23, `--list` = 95 — `grep -cE '^\s*test\('` counts 92 because line 1128 generates 4 tests per loop ; the 11 groups of dead selectors of §16 and 2.5/12.2 updated for the new shell), `widgets_integration.rs` 6 → **8** (`refresh_ms` round-trip of a legacy blob + persistence of a new value)** ; **+2026-09-15: removal of ALL auto-save — a single "Save" button in the rail footer (`[data-testid="settings-save"]`) empties each modified group via the shared store `stores/settingsDraft.svelte.ts` (`draft.*` draft + JSON baseline per group + sequential `saveAll()`) ; the 6 embedded cards lose their individual save buttons (their editable fields are in the same draft, the *actions* — scan, cert regeneration, rule add/delete — remain immediate) ; autostart/AI/widgets/weather go to the draft, pushed only on clicking Save ; header pill `settings-save-status` (unsaved / in progress / failure) ; E2E rewritten: 1.3, 3.4, 4.6, 5.3, 9.4 (comment), 12.3, 12.5, §15 "Auto-save" → "Save button" (15.1 the change alone does not PUT, 15.2 persistence via button + reopening, 15.3 in-flight status, 15.4 no PUT without editing, **+15.5 button disabled during saving**), 21.3 (no PUT before Save, one after), 23.7/23.8 (widget/interval toggles: staged then POST on click) ; +1 test (95 → 96)** ; **+2026-09-15 (SNMP/UPS monitoring): 3 startup WARN removed (`snmp.rs`/`ups.rs`: the `is_available()` predicates no longer log at warn — `debug!` ; `ufw` modeled presence-only in `dependencies.rs`, cf. Firewall guide). SNMP parser rewritten (see unit tests) + `snmpd` view extended to interfaces/storage ; `SnmpCard` card aligned on the backend fields (`bytes_in`/`bytes_out`, `mount_point`, `total`/`used`/`usage_percent`) + `formatBytes`. UPS: NUT configured in auto-detection (`MODE=standalone` + `usbhid-ups port=auto`, no model id hardcoded) by `install.sh` ; a UPS configured but not connected logs at `debug!`. Verified live on the server: `snmpd`+`nut-server` active, `/api/snmp/{availability,info,interfaces,storage}` and `/api/ups/{availability,list}` OK.** ; **+2026-09-17: `settings.spec.ts` **96/96 green** against the rebuilt binary (Firefox, `--project=firefox`) ; the only remaining red, **23.5** (section labels as micro-labels, `<= 12px`), is fixed — three sizes written `text-[var(--hv-text-xs)]` without the `length:` type hint compiled to `color:` and the labels inherited the 14px of their parent (detail, scope and verification pitfalls in the section "third bug of the same family" below). `npm run check`: **7793 files, 0 errors, 0 warnings**. The full run was killed by the kernel at 91/96 (machine memory pressure, not a failure) ; the last 5 (23.7–23.11) were re-run alone and pass — **5 passed (1.0m)**. **+2026-09-23: the "Save" button becomes the single save indicator** — the status pill visible above the button and the caption "N unsaved change(s)" below it are removed (component `SettingsSaveStatus.svelte` removed, import + footer snippet cleaned in `Settings.svelte`) ; the button exposes `data-state` = `clean` (blue `--hv-primary`, "Save") → `dirty` (red `--hv-error`, "Save changes") → `saving` (blue, disabled, "Saving…") → `saved` (green `--hv-success`, "Saved", held 2.5 s then back to blue) / `error` (red, "Not saved") ; the label and the icon **always** change with the color (WCAG 1.4.1 ; white on the 3 backgrounds: blue 6.15:1 / red 5.73:1 / green 5.14:1 — blue follows the `--hv-primary` accent re-hydrated at boot, whose shipped default is `#1E62AE` since 2026-09-23) ; an edit made during the green flash cancels it, a failing group parks the button on `error` (never "Saved"). `settingsBtnSuccess` added to `vocabulary.ts` (one skin per state, never two `bg-` on the same element) ; keys `settings.save_changes` / `settings.save_error` ×4 locales replacing `settings.unsaved_changes` ; `settings-save-status` kept but as **sr-only `role="status"`** (screen readers hear each state change, the rail displays only one element). E2E rewritten: helper `saveSettings` waits for `data-state = saved|clean` (a failure parks on `error` and therefore fails the wait), **1.3** on `data-state` + presence of the sr-only line, **15.3** verifies the 3 colors (`toHaveCSS` on RGB **read in the page** — `--hv-primary` is re-hydrated from the accent, the default token does not match the render) **and** the labels in blue → red → in flight → green → blue (pointer moved away before each background assertion, otherwise the hover reads `-*-hover`), **14.5** targets `.settings-data-actions .settings-btn-danger` (the rail button carries the same skin when a change is pending — strict mode otherwise). **96 tests unchanged** ; `npm run check` 0 errors. **2026-09-23 result: 96/96 green (9.4 min, Firefox, scratch instance `:8099`)** — the first shot was entirely red (96/96) because the Playwright binaries were absent from the machine (`npx playwright install firefox` required), the second 94/96 with two reds **outside the change**: **15.2** revealed a real cache bug (`put('/settings/<group>')` did not evict the `GET:/settings` aggregate with a 5 s TTL that the window re-seeds on reopening → fix `invalidateForMutation()` in `api.ts`, now covered by 15.2) and **15.3** compared against the hardcoded RGB of `tokens.css` whereas `App.svelte` re-hydrates `--hv-primary` from the accent (`#4a90d9`) → the RGB are now read in the page. **+2026-09-23 (standardized contrast)**: the default accent goes from `#4a90d9` (3.34:1 behind white **and** ≤3.34:1 as text on light backgrounds → §1.4.3 failure in both directions) to `#1E62AE` = `--hv-primary` (6.15:1 / 4.99–5.68:1), rewritten at each entry by `DEFAULT_ACCENT` + `normalize_accent` (default, persisted load, import, `PUT /settings/appearance` ; the accent has no editing UI, no operator choice is overwritten) ; brand gradient `--hv-app-accent-grad`: light stop `#4b8ad4` (3.57:1) → `#3275C4` (4.69:1) ; `theme-color` + manifest aligned ; **+1 unit test** (`normalize_accent`) and **+1 integration test** (legacy PUT → `#1E62AE`, aggregate included) → `settings.rs` 12→13, `settings_core_integration.rs` 12→13, **E2E settings 96 → 97 — **+2026-10-19 (Settings headers + wallpaper grid)**: the content header and every `SettingsCard` header drop their 36px icon tile — two lines of text only (title over its description), `px-6 py-2.5` / `py-2.5` instead of `pt-5 pb-4` / `py-4` — **measured 87.5 → 57 px** (content header) and **83.5 → 57 px** (card header) ; a plain `m-0` on the subtitle did not apply, `base.css`'s unlayered `p { margin-bottom }` outranks it, so it took `mb-0!` (the header measured 73 px until then — same family as the `!`-on-headings trap), and `SettingsCard` loses its `icon` / `iconVariant` / `accent` props ; **23.3 rewritten** (it asserted a 36px tile and its wrapper) to pin exactly 2 children, no icon, a header ≤ 64 px, the subtitle following the category and the header still not scrolling ; **4.7 extended** — the bundled thumbs are not all one shape (`hv-09` / `hv-13` ship 640×480, the other thirteen 640×427), so the tiles came out ragged; `.settings-wallpaper-thumb` now takes `aspect-[8/5]` (`h-auto` stays, the `height="200"` attribute is a presentational hint that would otherwise defeat `aspect-ratio`) and the test requires all 15 tiles to share one width and height — **measured live on the built CSS**: before 155×104 / 155×117 / 155×116 in the same row, after **155×97** at 3 columns and **148×93** at 2, every tile uniform. **+2026-10-19 (icon tiles)**: every decorative tile is one circle at one size — `--hv-settings-icon-tile` (28px) + `settingsIconTile` (`vocabulary.ts`) for the 11 Widgets rows and the 3 Privacy rows, the same token for the rail's rich tile in `WindowSidebarItem`, and the two icon-only buttons (AI refresh, firewall modal close) are circles too; three off-ladder glyphs went back to 16 (`PathPickerField` 14, MCP card 15 and 18). **23.12 added** — it walks all 8 categories, collects every tile host (`.hv-winsidebar-item-icon`, `.settings-icon-tile`) and every rendered `<svg>`, and requires one square box of the token's size, a radius ≥ half the box, and a glyph of 16 or 20 — **proven to bite**: reverted to the 36px/18px Privacy tile it fails `widgets: tile width … Received: 36`, reverted to `size={14}` it fails `security: glyph size 14 … Received array: [16, 20]`, green on the fixed tree (Firefox, Vite dev server `:5198` → backend `:8080`). Settings count 97 → **98**. **Verified live on the rebuilt binary** (`cargo build` + restart on `:8080`, so `include_dir!` served the new assets): content header 57 px with 2 children and 0 icons, the 6 security/monitoring card headers all 57 px with 0 icons, `23.3` / `4.7` / `2.1` / `12.1` / `13.1` / `22.1` green (Firefox, 6 passed). Counts unchanged (97). **+2026-10-19 (wallpaper batch): the bundled set grows 15 → 44 (`hv-16` … `hv-44` + thumbs), so 4.2/4.3/4.7 now assert 44 tiles; the thumbs span 3:2, 4:3 and 16:9 and the existing `aspect-[8/5]` tile still normalises them (4.7's uniformity check). Test count unchanged (98).** ; **+2026-10-22 (ROADMAP 13.1): 8.3 rewritten — the "Check for updates" button must hit `POST /api/settings/version/check` (the forced path), not the read-only GET the mount performs — and 8.4 added: mounting Settings issues no `/version/check` request at all (privacy baseline). The spec's `gotoDesktop` also gained the `dismissWhatsNew` helper the power/mcp/metrics/account specs already had: without it the once-per-version modal of a fresh scratch instance swallowed the dock click and the **first test of every run** timed out on `settings-page` (reproduced on 8.1 and 7.1, green on the second test of each run) — fixed, `8.` → 7/7 and the whole file **104/104 green (5.1 min, Firefox, scratch instance `:8099`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup`)**. `playwright test --list` per project = **104**. ; **+2026-10-23: 3.3 rewritten — the timezone picker renders the whole IANA database from `GET /api/settings/timezones` (`Europe/Lisbon` selectable, >200 options, region `optgroup`s), so the 14-zone shortlist cannot come back** ; **+2026-10-23 (ROADMAP 13 — the remaining cards)** : unit — `handlers/settings.rs` 17 → **28** (+2026-10-24 (ROADMAP 29.1): `mdns_is_enabled_by_default`, `saving_general_settings_applies_the_mdns_choice_to_the_host` (source guard: the save calls `crate::mdns::apply_enabled` on `spawn_blocking` and reports `mdns_notice`), `the_general_read_reports_the_live_mdns_flag`; 17 → 25 before that) (language normalisation accepts a region tag and refuses an unsupported one, `GeneralSettings::default().language`, e-mail address accept/refuse, webhook URL accept/refuse incl. credentials / non-http schemes / the 2048 bound, `EmailSettings` → `EmailConfig`), `settings_persistence.rs` 7 → **13** (language round-trip, `schema_version` on export, newer-schema refusal, legacy blob accepted, per-field import errors, valid import), `snmp.rs` 5 → **9** (v1/v2c only, v3 refused with a reason, the version reaches both argv builders, `iface` filter), `ups.rs` 7 → **14** (deadlines, `upsc`/NUT argv builders, `ups.conf` reset preserving the shipped section), `antivirus.rs` 8 → **11** (quarantine dir accept/refuse, `--scan-archive=yes|no` in the argv), `email.rs` 0 → **9** (generated msmtp config text/mode/argv, relay-first order, stderr tail), `notification_engine.rs` 0 → **2** (the shared client carries a deadline, a bad URL never reaches it), `antivirus_scheduler.rs` 0 → **4** (`is_due`: a broken cron is never due, a daily schedule due/not due, never-run due) ; integration — `settings_core_integration.rs` 19 → **24** (language persisted in the aggregate + export, `schema_version` on export, webhook URL refused then persisted exactly-trimmed, e-mail validation, import refuses a newer schema and names a bad field), `security_integration.rs` 25 → **26** (quarantine dir refused outside the roots, `scan_archives` round-trip), `snmp_integration.rs` 6 → **8** (v3 → `400` and nothing persisted, `iface` accepted) ; E2E `settings.spec.ts` **104 `test(` / 107 runtime** (+13.2 SNMP offers only v1/v2c and says why, +13.3 the UPS card surfaces `reload_warnings` with `role="status"`, +14.6 a partial reset names the failing areas) — *written, not run in this pass* ; **+2026-10-24 (ROADMAP 25): 104 → **106 `test(` / 109 runtime** — 3.5 mocks `PUT /api/settings/autostart` to `500 {message:"polkit refused: no authorization"}` and asserts a `role="alert"` with that exact reason, the rail Save parked on `data-state="error"`, and the switch reverted to the server-accepted value; 3.6 mocks GET/PUT to success and asserts the switch reflects the served `enabled` with no error band; 3.4's PUT assertion now accepts `200` or `500` (a host refusal is explicit since 25.2, not a silent 4xx)**
executed on 2026-09-23: 97/97 green (new guard **17.5**: the 8 category tiles of the rail
measured in both states — glyph/tint at rest, white glyph/flat fill when active — ≥3:1 §1.4.11,
RGB read in the page). **Palettes extended in the same pass**: avatars centralized in `$lib/utils/avatars.ts` (single source — `#4B8AD4`/`#8b5cf6` → `#3275C4`/`#7C3AED`, **white** initials also in Groups which painted the dark ink → 6/6 stops ≥4.5:1 ; guard `groups.spec.ts` **16.1**), disk ring + legend Storage (`#14b8a6`/`#f97316` → `#0F766E`/`#C2410C`, ≥3:1 §1.4.11 on white ; the "unpartitioned" gray `#e5e7eb` remains the *empty* value, next to its text label), Chat tile (`#14b8a6` → `#0d9488`, white glyph 2.49→3.74:1), **category accents** (the flat tile = state marker of the control, §1.4.11): widgets `#A78BFA`→`#7C3AED` (2.72→5.70), notifications `#F59E0B`→`#B45309` (2.15→5.02), ai `#06B6D4`→`#0E7490` (2.43→5.36), monitoring `#10B981`→`#047857` (2.54→5.48) — the other 4 (\\*-500) already passed (≥3.32:1), and **Login/Wizard scale** (l. 1): `--hv-login-accent` `#6366F1`→`#4F46E5` (4.47→6.29 as text), `accent-deep`→`#4338CA` (6.29→7.90), `soft-border` `#A78BFA`→`#7C3AED` (2.72→5.70 — it is also the text of the completed steps), logo gradients (stops 4.47/2.72 → 6.29/5.70 under the white digit) and button (4.47→6.29), focus ring following the new hue, guard `login.spec.ts` **13.6** → suite 94→**95** (`--list`) / 90→**91** (grep). |
| 27 | VirtualMachines | [x] **2026-09-10 — verified live** against a real Incus 6.0.0 (host with `qemu-system-modules-spice` + `incus admin init`) ; the only uncovered point: boot of a real Windows ISO (no ISO at hand). **2026-09-09: app rewritten from the ground up** (frontend↔backend contract realigned ; general-public wizard with **2 modes**: (a) curated OS catalog + presets + account seeded by cloud-init (SHA-512 hashed password, never in clear), (b) **installation from a Linux/Windows ISO** — file picker, empty VM, noVNC graphical screen via `raw.qemu -vnc 127.0.0.1:…`, Windows = NVMe disk + TPM 2.0 + Secure Boot + optional VirtIO drivers ISO) ; real waiting for the Incus operations ; tracked creation job ; xterm shell console (`tokio-tungstenite`) ; Images/Networks tabs removed. **Verified live end to end** against a real Incus (after `apt install qemu-system-modules-spice`): (a) containers — creation via the wizard/API, real xterm shell console, create/restore/delete snapshots, resize, deletion ; (b) **Linux ISO VM** — wizard → `create` → Incus creates the empty VM, `raw.qemu -vnc 127.0.0.1:20` + TPM/NVMe/SecureBoot applied, **the VM starts**, the Alpine ISO boots ; **the noVNC screen displays the Alpine boot console live in the browser** (canvas 1280×800, relayed RFB handshake, Ctrl+Alt+Del handle) ; `delete` stops then removes the VM. **1 bug found and fixed live**: Incus sends `state.disk: null` on a freshly started VM → `#[serde(default)]` failed (`invalid type: null, expected a map`) and broke the whole `/api/vm/list` ; helper `null_default` added (covers `state.disk` + `config`/`devices`/`expanded_devices`). Fixes to `install.sh` + `dependencies.rs` (spice package) added. **2026-09-10 — "fix everything" hardening**: (A) OVMF Secure Boot check before creation (`OVMF_VARS_4M.ms.fd` **confirmed present** on Ubuntu) + **TPM auto-repair** — if a VM with `vtpm` fails to start (Incus 6.0.0 does not wire swtpm), the job deletes the failed VM (waits for the op) then recreates it without TPM with a fresh VNC display → Windows-10-capable VM that starts ; (B) creation job turned into generic "preparation" ; (C) `ensure_readable_by_qemu` — `chmod o+r` on the ISO + `o+x` on each parent folder, error naming the blocking folder ; (D) `RESERVED_DISPLAYS` (mutex, 60 s reservation) against display collision on concurrent creation + auto-repair of the VNC port at `start` + 45 s timeout on the Incus requests (`/api/vm/list` can no longer freeze) ; (F) **ISO library** — download from a link (`POST /api/vm/isos/download`, progress job), "VirtIO drivers" button (`fedorapeople.org/.../virtio-win.iso`), `safe_iso_name`, deletion. **Verified live**: concurrent Linux + Windows ISO creation — both VMs RUNNING, displays 21/22 without collision, `win1` auto-repaired without TPM ; ISO download (67 MB) OK ; library chips + VirtIO button rendered in the wizard. Remains unverified (no Windows ISO at hand): real boot of a Windows ISO + 2nd VirtIO drivers CD-ROM ; real TPM 2.0 requires an Incus newer than 6.0.0 | [x] `incus.rs` (**22** — + `null_default`: `state.disk`/`config`/`devices` as explicit `null` tolerated ; **+3 on 2026-10-13 (b): the catalogue grew Windows 11/10 + minimal Ubuntu/Debian entries with the `iso_only` flag, and `audio_device()`**; **+2026-10-23 (ROADMAP 19.1): `#[test]`/`#[tokio::test]` count 13 → 22 — `operation_id_refuses_an_async_reply_without_an_operation` (an async reply with no `operation` used to answer `""` = "already done"), `classify_operation_reads_the_async_status_codes` (200/106 success, 100/101/103/104 still running, 105/107/999 failure, `err` wins), `classify_operation_never_invents_a_failure` (a **sync** reply has no operation status and must be `Succeeded` — reading the missing code as 0 is what reported successful creates as failed), `wait_budget_outlives_the_operation_timeout` (the 900 s create wait must never be cut at the 45 s default), and four `/wait` tests driving the real transport over a throwaway Unix socket (`fake_incus`: `wait_reports_completed_on_an_async_success`, `wait_reads_a_synchronous_reply_as_completed`, `wait_reports_still_running_when_the_deadline_expires` — `StillRunning` with the id, bounded by its deadline, not an error —, `wait_keeps_a_real_failure_a_failure`, `wait_on_a_missing_socket_is_not_available`)**) + `handlers/vm.rs` (**29** — +8 on 2026-10-24 (ROADMAP 32.1/32.2): the liveness tracker starts fresh and `touch` resets it, `>=` at the deadline, the ping/deadline ordering allows three missed pings, the declared-length classification (below/at/above the floor, above the ceiling, absent), and a source guard asserting the import path uses `MIN_ISO_BYTES` rather than a literal ; 21 — +`vnc_display`/`has_screen`, `validate_local_iso` (absolute/traversal/extension/size/`chmod o+r`), `alloc_vnc_display` in the range 20–119, `safe_iso_name`, `human_bytes`, capacity probes (`swtpm`/OVMF) without panic ; `valid_instance_name`/`valid_unix_username`/`valid_snapshot_name`, `size_preset`, `normalise_status`, `parse_size_to_mib`, `instance_to_info` maps the wire contract, `pretty_os` fallback ; **+3 on 2026-10-13 (b): `apply_audio_device` attaches `audio0` only for a VM, and an `iso_only` catalogue entry is rejected on the ready-made path**; **+2026-10-23 (ROADMAP 19.2): `#[test]`/`#[tokio::test]` count 15 → 21 — `test_alloc_vnc_display_in_range` is now `#[tokio::test]` and releases its slot, `test_alloc_vnc_display_never_hands_out_the_same_slot_twice` (three concurrent allocations, no duplicate — the slot is claimed before it is probed), `test_vnc_reservation_covers_a_whole_create` (the claim must outlive `CREATE_WAIT_SECS`, not the old 60 s), `test_display_is_free_sees_a_bound_port`, `test_blocking_io_passes_values_and_messages_through`, `test_read_iso_entries_lists_only_isos`, `test_finalise_iso_rejects_and_removes_a_too_small_file` (the 10 MB floor still bites and the rejected body is removed)**) | [x] `vm_integration.rs` (**28** — +2 on 2026-10-24 (ROADMAP 32.2): an unusable download link is refused before dialling (non-http scheme, non-`.iso` name) and the route is admin-only ; 26 — **+2 on 2026-10-13 (b): `create` with `audio: true` attaches the device, and a Windows `iso_only` OS is refused on the ready-made path**; missing auth (400)/bogus (401)/non-admin (403) on **all** the endpoints (incl. `screen/ticket`) ; admin reaches each handler without 404/405 ; `available`/`os-options` without Incus ; `list` = clean message without socket leak ; `create` validation **before** Incus: name, unknown OS, unknown kind, short/absent password, invalid login, alpine as VM, **+ ISO: relative path/wrong extension/missing file, mutually exclusive ISO+OS, neither one, valid ISO → reaches Incus** ; `config` (bounds, empty body) ; snapshot name ; console **and** screen ticket require a running machine ; **+2026-10-23 (ROADMAP 19.2): 24 → 26 — `create_iso_still_names_why_the_file_is_refused` (a directory ending in `.iso` and a file under 10 MB each keep their own sentence through the `spawn_blocking` hop) and `iso_store_lists_and_deletes_through_the_blocking_pool` (`GET /api/vm/isos` lists only `.iso` files with their size, `DELETE` really removes the file, a second delete says "not in the store")**) | [x] `vm.spec.ts` (45 `test(` / 47 runtime, 14 categories ; **+2026-09-12: Appliance design migration — `PageLayout` (machine list) + detail drawer + catalog/ISO wizard in Tailwind-on-tokens, console/screen overlays kept, selectors `data-testid="vm-*"` (wizard → `[data-slot="dialog-content"]`), zero scoped CSS** — 3 initial states ; catalog **and ISO** wizard (mode toggle, Linux/Windows toggle, 1 vs 2 file pickers, no password field in ISO, blocking without ISO) through to "Cancel" ; backend contract by `fetch` (`os-options`, `list` shape incl. `has_screen`, `create` rejections catalog **and ISO**, screen ticket) ; detail drawer ; xterm console ; responsive 360/768/1920 ; a11y ; security (no secret exposed, non-admin rejected on all endpoints incl. `screen/ticket`, XSS name not reflected) ; keyboard ; network ; **real opt-in cycle `VM_E2E=1`** ; memory regression) ; **+2026-09-13: 2-pane master-detail — the detail drawer (formerly `fixed` portaled) becomes an inline 420px column to the right of the list (`ResizeObserver` split < 720px → a single pane), `vm-drawer`/`vm-d-specs`/`vm-d-danger` kept, Escape/X close preserved (`vm.spec.ts` 5.1/5.2 unchanged)** ; **+2026-10-13 (b): the ISO-only catalogue entries, the audio checkbox and the "open screen in a new tab" action are new. Spec not re-run** ; **+2026-10-23 (ROADMAP 19.1): 43 → 45 `test(` (47 runtime) — section 15 "Long Incus operations", fully stubbed so it needs no Incus: 15.1 a `start` whose wait expired shows the server's "still starting … (operation …)" sentence in the success banner (it used to be a red "Incus did not respond in time"), 15.2 a genuinely refused action still shows the server reason** |

| 28 | Scheduled Tasks | [x] 2026-10-13 (c) — new screen, live-audited during its E2E run on a scratch instance (`:8091`): launcher opening, create/edit modal, run-now on `/bin/true`, disable/enable, delete-with-confirm, non-admin bounce ; **+2026-10-22 (ROADMAP 12.1): the overlap guard is real** — a manual run and a scheduler tick now share one process-wide `RUNNING` set (`TaskRunGuard`, released on drop), so the same task can no longer run twice; "Run now" answers `409` while a run is in progress ; **+2026-10-23 (ROADMAP 12.2/12.3/12.4): the cron is evaluated in the task's IANA timezone (blank = appliance timezone, applied to the host with `timedatectl`), an unparseable cron pauses the task after one recorded failure, and "Run now" answers `202` + a run id while the execution continues outside the request** | [x] `scheduled_tasks_scheduler.rs` (**12** — `TaskAction` validation matrix: absolute-path/no-shell/whitespace, device traversal, test-type enum, power whitelist, pool-name charset; `reference_time` 8-day cap ; **+2026-10-22 (ROADMAP 12.1): `a_task_cannot_run_twice_at_once` — the second claim is `Busy` and names the task, another task is unaffected, the claim is released on drop ; `manual_and_scheduled_paths_share_one_registry` — the claim taken by one path is seen by the other, both ways ; **+2026-10-23: `parse_timezone_accepts_iana_names_only`, `a_cron_is_evaluated_in_the_tasks_timezone` (winter vs summer Paris, appliance-zone fallback), `an_unknown_timezone_or_cron_is_an_error`, `weekday_normalisation_survives_the_timezone_hop`**) + `storage/scheduled_task_repo.rs` (8 — **+2026-10-23: timezone round-trip, `set_enabled` keeps the run summary, `pause_with_error` disables and clears the next run, `close_interrupted_runs` closes only `running` rows**) + `power.rs` (5 — `PowerAction` parse/verbs, governor metacharacter rejection, hdparm `-S` code mapping (1-20 min → ×12, 21-330 → 241-251, >330 rejected), `hdparm_args`, device-name basenames) + `handlers/scheduled_tasks.rs` (4 — **+2026-10-23: timezone canonicalisation and the next run precomputed in the task zone**) + `setup.rs` (**+2026-10-23: `test_timezone_grant_is_scoped_to_set_timezone` — the sudoers grant names `set-timezone *`, never the bare binary**) | [x] `scheduled_tasks_integration.rs` (**8** — CRUD cycle, auth matrix (anon 401-shape / non-admin 403 / admin 200), validation rejections (bad cron, payload/type mismatch, unknown type, non-absolute command, missing binary), **real run-now of `/bin/true` then `/bin/false` through the router** with run-history assertions, unknown-id shapes ; **+2026-10-22 (ROADMAP 12.1): `run_now_is_single_flight_against_itself` — a `/bin/sleep 1` run holds the task, the concurrent second POST is `409` with "already running", exactly one run row is written, and the claim is released so the next start succeeds ; **+2026-10-23 (12.4): `run_now_answers_202_and_records_history` — `202` + run id, the row then reaches `success`, and a failing command is recorded as a run failure, not a request error ; `timezone_is_validated_and_stored_per_task` ; `enable_toggle_keeps_the_run_summary`**) + `power_integration.rs` (9 — admin-only on every endpoint incl. host `reboot`, unknown action rejected without executing, governor values not offered by the kernel rejected, spindown device/range validation + save/list/clear round-trip) | [x] `tasks.spec.ts` (11 `test(` / 22 runtime across chromium+firefox — launcher, create, inline validation error, edit repopulates per-type payload, run-now flips status + runs drawer, disable/enable, delete confirm, non-admin cannot open ; **+2026-10-23: `2.3` timezone picker round-trip — the field is a `<select>` over `GET /settings/timezones` (`Europe/Lisbon` present, region `optgroup`s), it stores `Europe/Paris` and the row shows it, `4.2` asynchronous run-now (`Running` → `Success` with no reload), `4.3` an argument containing a space stays one argv entry**) |
| 29 | Metrics | [x] 2026-10-13 (c) — new screen, live-audited during its E2E run on a scratch instance: five chart cards, range toggle group, empty state on a fresh DB ; **+2026-10-23 (ROADMAP 11): null semantics — a failed `/proc` read is a gap, not a `0.0`** (migration `056_metrics_nullable_series.sql` rebuilds `metric_samples` with every series nullable; each series averages on its own inside a bucket; `latest` answers `200` + `data: null` on a fresh install instead of `400`; the screen decides its empty state on all six series, drops a stale range answer, and prints `—` for a missing figure) | [x] `metrics.rs` (**19** — 2026-10-23, was 5: + per-series averaging and an all-null bucket, + `sample_from` (a failed read is `None` never `0.0`, a wholly failed tick is all nulls, the rate divides by the real elapsed time — 1 200 B over 120 s is 10 B/s, no previous instant / counter reset / frozen clock are `None`, CPU delta vs since-boot fallback), + the five `/proc` parsers on fixtures; the tolerance test now reads through `read_counters`, which reads each file once) | [x] `metrics_integration.rs` (**8** — 2026-10-23, was 5: the empty-DB `latest` is now `200` + `data: null`, + a stored `NULL` sample is a gap while a stored `0.0` stays a zero, + a null series does not drag down its neighbours in the same bucket, + `latest` keeps a null series null) | [x] `metrics.spec.ts` (**6 `test(`** — 2026-10-23, was 4: §5.1 stubs a temperature-only dataset and requires the charts, not "no data", with `—` for the series nobody measured; §5.2 stubs a 1.5 s `24h` answer and requires the newer `1h` figure to survive it. **Written on 2026-10-23; not run yet (per instruction)**, so the count is the test functions added, not a green run; the earlier 4 were 3 passed / 1 conditional skip on chromium, 3/3 firefox) |

### Notable sub-features (integrated into a screen above)

| Sub-feature | Manual audit | Rust unit tests | Rust integration tests | Playwright E2E tests |
|---|---|---|---|---|
| **Typography — native text ladder (cross-cutting)** | [x] 2026-10-18 — every rendered text size resolves to a platform desktop text style (11/12/13/15/17/20/22/26/34, plus the documented decorative 48/56/62/72), verified live on the login screen and five apps ; **+2026-10-25 (ROADMAP 34.1): the ladder honours the reader's text size** — every token moved from `px` to `rem` against `html { font-size: clamp(100%, 100%, 20px) }`, so raising the browser's configured font size scales the whole product up to a documented 1.25× ceiling. Before this the product rendered identically at a 16px and a 32px root (Apple HIG *Layout → Adaptability*, WCAG 1.4.4). Three login defects found by the same HIG review were fixed with it: the field box was `h-11`, a **ceiling** that clipped the 16.25px text a 1.25× reader gets (now `min-h-11`, 44px touch minimum kept), the "Forgot password" link had no hit area (19.9px tall, `padding: 0` → `h-11`), and the gateway card width alternated between two values for one viewport (→ deterministic) | n/a (tokens only) | n/a | [x] `design/typography.spec.ts` (**11 `test(` / 15 runtime** — 1.1/1.2/1.3 login ladder + role table + 11px floor; 2.1 desktop shell; 2.2 window title 15; 2.3.{settings,users,dashboard,storage,network} ladder per app; 2.4 Settings roles; **§6 "Text scaling" (2026-10-25): 6.1 every step scales with the root and is exact at 100%, 6.2 nothing is visually clipped at the 1.25× ceiling and the card grows instead, 6.3 the card width is deterministic across mounts, 6.4 `html` declares a `clamp()` rather than a px literal**). Walks the live DOM and fails on any element rendering its own text off-ladder; **verified by mutation twice** — setting `--hv-type-body` to 14px turns 1.1/1.2/2.3 red, and setting it back to `13px` turns 6.1 red with `body at root 20px / Expected: 16.25 / Received: 13`. Run live 2026-10-25: **15/15 green** (Chromium, rebuilt release on `:8080`) |
| Security suite — Antivirus/Anti-Ransomware/Firewall (in Settings) | [x] 2026-07-16 (+ 09-06: real EICAR scan + real honeypot ; **+2026-09-09: folder paths = choice only, no more manual entry — new `theme/PathPickerField` + `theme/FloatingPathPicker` (floating non-modal movable panel, `use:portal` to `<body>`), standard reusable component (`mode="folder"\|"file"`, sidebar of locations — physical/USB/RAID/cloud/network disks, each displayed by its FS label or brand+model+serial no. + size — + list/grid toggle persisted in `localStorage`) ; `handle_explorer_disks` (frozen) enriched with `label`/`vendor`/`model`/`serial` (1 `lsblk -P` pass) ; `PathField`/`FolderPicker` (modal) unchanged**) | [x] `firewall.rs` (**31** — 2026-10-21, was 9: every `ufw show added` rule form, the comment/keyword trap, the default policy, all six validators, `sshd` port detection incl. drop-ins), `geoip.rs` (**4** — 2026-11-05, P1-14: strict country/action, database/CIDR parsing, plan cap and managed-comment format), `ransomware_watcher.rs` (7), `antivirus.rs` (8 → **11** on 2026-10-23, ROADMAP 13: quarantine-dir validation + `--scan-archive` argv), `ransomware.rs` (3) | [x] `security_integration.rs` (**27** — 2026-10-21, was 18 ; **+2026-10-23**: quarantine dir refused outside the managed roots, `scan_archives` round-trip ; **+2026-11-05 (P1-14)**: bad country/action refused before any command, missing GeoIP database → `503` named, status remains readable) | [x] `security.spec.ts` (**41** `test(` / 44 runtime — 2026-10-21, was 30: §5 gains the disable gate, the verb-smuggling and malformed-input rejections, the 404 delete, the default-policy validation and the history envelope; new §5b (4) mocks the firewall endpoints so the card's default policy, focus-trapped remove dialog, `spec` payload, disable confirmation and error feedback run without `ufw` — 2.4: the "Select a folder" button opens a `.hv-fpp` non-fullscreen panel, no `.hv-fp-overlay`, drag-handle, sidebar of locations (disk row = size/serial no.), list/grid toggle (persisted), closes on Escape ; 2.2 renamed: `#avScanPath` field `readonly`; **+2026-11-05 (P1-14): 5.15 mocks the GeoIP status/set endpoints and proves the card posts `{country:'FR', action:'allow'}`**) |
| Transfer quotas (in Users/Groups/Explorer) | [x] 2026-07-18 (+07-21 overview ; **+2026-10-23 (ROADMAP 23)**: fail-closed guard, calendar counting periods, self-or-admin reads — written, not run) | [x] `transfer_quota.rs` (**31** — +6 on 2026-10-24 (ROADMAP 33.1): a fresh entry hits the cache, an expired one misses (TTL boundary exclusive), the cache is keyed per username, `invalidate_group_cache` drops one account only, pruning keeps fresh entries, and the TTL must be short *and* documented ; **2026-10-23, 9 → 25**: the period matrix and its named error, day/week/month calendar windows, December rollover, Paris vs UTC, a DST boundary, the elapsed-window matrix, the reset instant, `bytes_to_i64` saturation, the fail-closed message ; the 9 historical `check_quota`/`Display` tests kept) + `storage/transfer_quota_repo.rs` (**9**, 2026-10-23 — period + window round-trip, a period change zeroes the counters while a limit edit does not, `reset_window`, `reset_expired` touches only its own period, the counters floored at 0, a no-op on a subject with no row) + `handlers/explorer.rs` (**+1** source guard: the only two quota loads in the file are inside `load_quotas_or_refuse`, so 23.1's fail-open cannot come back) | [x] **`tests/quotas_integration.rs` (15, recreated 2026-10-23)** — the file was deleted on 2026-09-09. Auth scoping (a member is refused the list, another user's quota and every group quota, and can still read their own), an unknown period refused with nothing stored, a period round-tripped with its window and next reset, a period change zeroing the counters vs a limit edit keeping them, an elapsed window settled by the guard with **no** scheduler tick, the reset pass leaving an open window alone, delete → the disabled default, a negative limit refused ; plus the historical `explorer_integration.rs` quota guards (download 429 / upload) | [x] `quotas.spec.ts` (**35 → 40** `test(` — §15: the period label + reset line on the seeded row, the period and instants the API supplies, a lifetime row with no reset date, a member refused the list and another user's quota, a member reading their own) + `users.spec.ts` 5.8 and `groups.spec.ts` 5.6 **extended** (counts unchanged): the counting-period select is saved and the reset line appears, and the stored `period` is read back over the API. Written on 2026-10-23; **not run yet** |
| Host power card (Settings → System) + disk spindown | [x] 2026-10-13 (c) — live during the E2E run; the reboot/shutdown/suspend buttons are **never confirmed** in tests (host safety): presence + cancel path only ; **+2026-10-24 (ROADMAP 24)**: the desktop power flow is verified with `POST /api/server/*` mocked — refusal → `role="alert"` with the host reason, acceptance → `role="status"`, no double submit | [x] covered by `power.rs` (**6** — parse/verbs, the single `sudo systemctl` argv for every action, governor validation, hdparm mapping/args, device names) + `handlers/system.rs` (**4**, new 2026-10-24 — the shared primitive guard, the status labels, the accepted and refused response bodies) + `storage/disk_power_repo.rs` (2 — upsert/list, empty setting deletes the row ; **+2026-10-23**: `delete_all()` is what Settings → Reset uses, and `hdparm`/`systemctl` now carry a 30 s deadline) + `setup.rs` (**+1**: `test_power_grant_names_the_systemctl_verbs_only` — the sudoers grant is exactly `reboot`/`poweroff`/`suspend`, never the bare `shutdown` binary) | [x] `power_integration.rs` (**9**, was 4, see row 28 — **+2026-10-24**: one admin gate across `/api/server/*` and `/api/power/*`, an invalid session refused on every power route, the server routes POST-only, and the refusal/accepted bodies produced by the mapper both handlers use. **No test POSTs as an admin to a power route**: that would take the host down, so the response body is asserted through the exported seam rather than through a live call) | [x] `settings/power.spec.ts` (4 `test(` / 8 runtime — **3 passed / 1 conditional skip (hdparm absent)** — three action buttons render, confirm + cancel keeps the host alive (`/api/auth/me` still answers), governor select vs. unsupported note, spindown add/remove row) + **`desktop/power.spec.ts` (5 `test(` / 10 runtime, new 2026-10-24)** — refusal alert with the server reason + retry, accepted restart announced as `role="status"` with exactly one request, 44px labelled dismiss with a visible focus ring, a dropped connection never reported as a refusal, 400px width with no horizontal overflow ; every `POST /api/server/*` is intercepted by `page.route`, **never a real reboot** |
| Notification events + webhook sink (Settings → Notifications) | [x] 2026-10-13 (c) — live during the settings smoke | [x] `email.rs` (**9**, 2026-10-23 — the saved SMTP settings generate a `0600` msmtp config, relay-first order, bounded stderr) + `notification_engine.rs` (**2**, 2026-10-23 — shared client with a deadline, an unvalidated URL is never dispatched) + `notification_events.rs` (4 — catalog ids unique + the 8 health checks covered, toggles default to enabled, Slack/Discord/generic payload shapes, strict format parsing) | [x] `notifications_integration.rs` (**26** — **+8 on 2026-10-23 (ROADMAP 21.2/21.3)**: per-user read-state isolation, caller-scoped `mark-all-read`, admin-only delete + clear-all gates, `404` on unknown id read/delete, pagination `limit`/`offset`/`total`, clamped oversized limit — *written, not run*; the earlier **+4 on 2026-10-13 (c)**: events-config admin gate, catalog defaults all-enabled, mute persists + unknown id rejected, `webhook_format` round-trip + strict validation) | covered by `settings.spec.ts` (the card is part of the Settings screen; not re-run in this pass) |
| Snapshot schedules + GFS retention (Storage → Snapshots tab) | [x] 2026-10-13 (c) — live during the E2E run on a scratch instance; never touches a real pool (run-now targets `e2e-no-such-pool`) | [x] `snapshot_scheduler.rs` (13 — auto-name parse (and rejection of manual/other-prefix names), tier = youngest window, per-generation keep, N generations per tier, zero-policy deletes the tier, aged-out snapshots move to older tiers, ISO-week bucket, **derived tier windows follow the counts**, **hourly retention can exceed a day**, **one run per schedule at a time**, **one run per pool at a time**, **only the transition into failure notifies**) + `snapshots.rs` (6 → **15** on 2026-11-05 (P1-12/P1-13) — size parser, **backend follows the pool type not the installed binaries**, **mdadm pools resolved by the filesystem on top**, **`/proc/mounts` fstype lookup**, **+2026-10-24 (X.2): the source guard + the deadline ordering**; **+6 per-share**: deepest ZFS dataset of the pool, Btrfs confinement + hashed slug (`data/photos` ≠ `data-photos`), restore-path validation (`..`/absolute/NUL), snapshot-name token, ZFS/Btrfs browse paths, and the per-share list parsers ignoring child datasets / other shares); **+3 Btrfs layout/rollback**: `subvol=@` vs legacy option parsing, `/proc/mounts` source lookup, and the pure rollback plan + recovery commands with safe distinct subvolume names) + `storage/snapshot_schedule_repo.rs` (3) | [x] `snapshot_schedule_integration.rs` (9 → **10** — CRUD cycle with GFS counts, admin-only matrix incl. run/delete, pool/cron/prefix validation, pool-snapshot list validation, run-now on a missing pool records `last_status='failed'` + `last_error`, unknown ids fail cleanly, **failed prune is a failed run + notifies once** (fake `SnapshotBackend`), **duplicate `(pool, prefix)` → 409**, **pool snapshot list readable by any signed-in user**, **+2026-11-05 (P1-12): a half share/share_type pair is refused before any read and the historical pool-wide body still stores both fields NULL**) + **`share_snapshots_integration.rs` (3, new 2026-11-05)** — read needs a session, mutations need an admin, and the validation gates (unsafe snapshot name, missing restore confirmation, `..` traversal, unknown share `400` never `500`) run before any ZFS/Btrfs command | [x] `storage/snapshot-schedules.spec.ts` (5 → **7** — tab opens (empty or cards), create through the modal, **duplicate pool+prefix refused inline in the modal**, run-now on a missing pool surfaces the error on the card, delete via confirm dialog; **+2026-11-05 (P1-12): 5.1 a share target posts `share`/`share_type`, 5.2 the restore action stays disabled until path + overwrite confirmation and posts the exact payload**) + `storage.spec.ts` 1.3 updated **6 → 7 tabs** (full Storage suite re-run green: 37 passed / 3 conditional skips) |

**2026-09-13 (transfers v2)** — download: reservation under per-subject lock + refund if aborted, download ticket (check quota before the stream), `Range` (only the bytes of the range are reserved) ; multipart and chunked upload: per-subject lock held throughout the transfer + re-check under lock at completion (TOCTOU anti-race).

| **DHCP server (Settings → Security) + Local DNS server (Settings → Security)** | [x] 2026-11-05 — phase C1 added the local DNS half; the two cards are one `dnsmasq`, verified live on a scratch instance (`PORT=8311`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup`) with every API call intercepted, so nothing was written under `/etc/dnsmasq.d` and the daemon was never restarted | [x] `dhcp_server.rs` (**15** — +5 for `render_conf`/`daemon_wanted`) + `local_dns.rs` (**22**) | [x] `dhcp_server_integration.rs` (21, unchanged) + `local_dns_integration.rs` (**14**) + the two new routes in `phase4_auth_integration.rs` (6) | [x] `settings/dhcp.spec.ts` (8, green) + `settings/local-dns.spec.ts` (**12/12 on Chromium and Firefox**) |
| **Generic reverse proxy (Settings → Security)** | [x] 2026-11-06 — phase C2: one host-routed listener (default 80), SQLite rules, private-address-only upstreams, cross-host one-time-ticket gate; live on a scratch instance (`PORT=8311`, `HOMEVAULT_REVERSE_PROXY_PORT=8391`) against a loopback upstream, including the full `/proxy-auth` → callback → `hv_app_session` round-trip and ticket replay refusal | [x] `reverse_proxy/mod.rs` (**21**) + `proxy.rs` (**6**) + `ticket.rs` (**4**) | [x] `reverse_proxy_integration.rs` (**21**) — CRUD/auth/validation plus a real listener forwarding to a loopback upstream | [x] `settings/reverse-proxy.spec.ts` (**11/11 on Firefox and Chromium**, real backend, rules cleaned up) |
| **Router port forwarding (Settings → Security)** | [x] 2026-11-06 — phase C3: UPnP-IGD mappings through `upnpc`/miniupnpc, read-only discovery first, admin-only confirmed add/remove, private-client-only policy, bounded calls; no real router on the test host so every E2E call is intercepted | [x] `upnp.rs` (**16**) + `handlers/upnp.rs` (**1**) | [x] `upnp_integration.rs` (**9**) — access-control matrix and every refusal before `upnpc` runs | [x] `settings/upnp.spec.ts` (**9/9 on Firefox and Chromium**, intercepted) |
| **Web Push notifications (Settings → Notifications)** | [x] 2026-11-06 — phase D2: VAPID keys sealed at rest, per-device subscriptions with explicit browser consent, RFC 8291 `aes128gcm` on `ring`, sender in `notification_engine`, per-device revocation and expired-device cleanup; live key generation on the scratch instance (0600, `hv1:` sealed) | [x] `web_push/crypto.rs` (**10**) + `web_push/mod.rs` (**6**) | [x] `web_push_integration.rs` (**10**) | [x] `settings/web-push.spec.ts` (**7/7 on Firefox and Chromium**, Push API scripted) |
| **Outbound HTTP proxy (Settings → Security)** | [x] 2026-11-05 — P1-8: one persisted setting applied to ACME, DDNS, version/app-image checks, webhooks/SMS and Web Push; loopback is always exempt, credentials in a proxy URL are refused, SOCKS is refused with a named reason (this build has no SOCKS feature) | [x] `proxy.rs` (**10** — URL scheme/credential/query refusals, enabled-vs-disabled validation, `no_proxy` shape incl. the `*.` normalisation, always-direct merge/dedup, empty-when-disabled proxy list, serde defaults, trimming) + `handlers/proxy.rs` (**2** — payload carries the loopback exemptions and never a credential) | [x] `proxy_integration.rs` (**8** — 401/403 matrices, default `effective_no_proxy`, trimmed round-trip, five validation refusals, disabled-empty save, a real mock proxy receiving an absolute-URL request, and loopback bypassing the proxy) | [x] `settings/proxy.spec.ts` (**9 `test(` / 9 runtime** — card fields, loopback exemption, SOCKS and empty-enabled refusals, disabled save round-trip, keyboard, secret hygiene, 360px overflow, labels; **written, not run**) |
| **Content search (Settings → System)** | [x] 2026-11-06 — phase D1: opt-in FTS5 index over chosen Explorer roots, text allowlist, bounded walker (50k docs / 256 MiB / 1 MiB per file), ACL filter before snippets, 30-min refresh + rebuild, retention and delete-my-index; live probe indexed 2 files, searched and wiped them | [x] `content_index/mod.rs` (**14**) + the migration-parser test (**+1**) | [x] `content_index_integration.rs` (**9**) — FTS triggers, retention, admin gate, refusals, delete, ACL | [x] `settings/content-index.spec.ts` (**7/7 on Firefox and Chromium**, intercepted) |
| System dependency inventory ↔ `install.sh` | [x] 2026-09-06 (missing packages added: ddrescue, testdisk, wireguard-tools, openvpn, restic, syncthing, cryptsetup, dmidecode, ethtool, snmp) ; **+2026-09-23: missing packages added — `git` (Docker app catalog), `unzip`/`zip`/`7z` (Explorer-Browser compression/extraction), `pciutils` (GPU card), `hostname`, `python3` (PAM login)** ; test parser extended to single-line `install_packages` (packages after the closing quote of the label — e.g. `ffmpeg`), 7 `System` entries added to `specs()` ; **+2026-09-23: `7z` installed adaptively (`7zip` on Debian 12+/Ubuntu 23.04+, `p7zip-full` otherwise) — `apt-get install` fails all-or-nothing if ONE package of the batch is unavailable, so the archive group no longer crashed as a whole ; `install_packages` now has a package-by-package fallback + `apt-cache policy` queries with `LC_ALL=C` (localized label `Candidat` ≠ `Candidate`)** ; **+2026-09-23: `install.sh` restructured into 5 "curl-pipe" style steps — the application archive is now downloaded by the script (`HV_RELEASE_URL`, or `$1` for a local file, no more mandatory argument), the systemd service and udiskie automount are **embedded** (a piped `curl | sudo bash` has no checkout on disk) with fallback to `installer/homevault.service` / `installer/setup-automount.sh` when the repository is present, Step 4 = global verification (packages + service + HTTP response + binary location), Step 5 = final message in English + local/internet addresses** ; **+2026-10-12: `installer/` reduced to ONE single file, `install.sh`, self-contained — `homevault.service`, `setup-automount.sh`, `optional-packages.conf` and the packaging `Makefile` removed (the systemd unit and the udiskie config are written in clear in the script, no more dependency on a neighboring file or on `$0`) ; the binary is downloaded per architecture from the GitHub Releases (`homevault-<arch>.tar.gz`, `HV_RELEASE_URL`/`HV_RELEASE_SHA256`/`$1` overridable, optional SHA-256 check) ; new maintainer mode `install.sh --package` (frontend + backend build → `installer/release/homevault-<arch>.tar.gz` + `.sha256`) which replaces the `Makefile` ; corrected data tree (no more `wallpapers/`, addition of `logs/`, `quarantine/`, `vm/isos/`)** | [x] `dependencies.rs` (**21** — **+2026-10-25: `test_documented_shadcn_components_match_the_tree`**, the doc↔code guard for the component kit in both directions: every directory under `frontend/src/lib/components/ui/` must be named in the design-system guide's §4 Framework row, and that row must not name a component the tree does not vendored. It exists because the guide advertised **30** generated components while **11** were on disk and **0** of the 19 extra ones was imported anywhere — the doc described an intention, the same class of false invariant as ROADMAP X.4. **Proven to bite both ways**: re-adding `` `progress` ``/`` `tooltip` `` to the row fails with *"the Framework row names components that are NOT vendored under ui/"*, and dropping `` `spinner` `` from it fails with *"vendored under ui/ but not listed"* ; each package of the inventory must appear in `installer/install.sh` ; `test_install_sh_embeds_a_complete_systemd_unit` and `test_install_sh_embeds_the_automount_config` lock the useful directives of the systemd unit and of the udiskie config written by the script ; `test_install_sh_is_self_contained` forbids any reference to a removed neighboring file ; **+2026-10-19: six entry/interface invariants, because no test drives the shell itself — `test_install_sh_never_reads_prompts_from_stdin` (no `read -r -p` without `-u 3`), `test_install_sh_binds_a_terminal_for_prompts` (fd 3 on `/dev/tty`, probed by *opening* it), `test_install_sh_tui_keeps_answers_off_the_terminal` (`--output-fd 1` + fd 4 + TUI stdout to the log), `test_install_sh_exposes_the_interface_flags` (`--tui`/`--cli`/`--yes`/`--help`, and `--yes` beating TUI detection), `test_install_sh_elevates_through_sudo_and_forwards_release_knobs` (terminal relaunch, `exec sudo`, the four `HV_RELEASE_*` knobs forwarded, never `sudo -E`, `--package`/`--dry-run` excluded) and `test_install_sh_reexec_cannot_loop` (`HV_INSTALLER_STAGE`)** ; **+2026-10-19 (dependency completeness + English-only): `test_every_commanded_binary_has_an_install_source` walks `src/`, collects every binary passed to `Command::new("…")` and `resolve_bin("…")`, and fails on any name that is neither probed by this inventory nor listed in `BASE_BINARIES` (Priority: required packages) / `BINARIES_WITH_DEDICATED_CHECK` (module predicates, external GPU drivers) — it found `hdparm`, `wpa_cli`, `mkfs.xfs`, `mkfs.exfat` and `avahi-browse` missing from `install.sh`, and is proven to bite (renaming the `hdparm` probe fails it) ; `test_install_sh_is_english_only` (no accented letters) and `test_install_sh_pins_an_english_locale` (`export LC_ALL=C`) pin the English-only rule, whiptail buttons included**) ; **+2026-10-19 (Ollama/Docker provisioning): `test_install_sh_docker_compose_never_breaks_apt` — the Docker repository lives in a best-effort `add_docker_repo()` (guarded `curl | gpg`, guarded `apt-get update`, list file removed on failure), `docker-compose-v2` is preferred over the third-party repo, and Step 4 checks the Docker *daemon* (`docker info`), not just the CLI ; `test_install_sh_installs_ollama_for_the_chat_app` extended — `zstd` is installed before Ollama's installer and its presence is checked, because Ollama ships a `.tar.zst` and its installer hard-fails without it (Priority: optional, absent from a fresh host)**) | n/a | n/a |

## Remaining to test as a priority (no automated test at all)

**2026-10-21 — the Rescue destructive paths need a real host pass.** The
validators, the option rejections, the confined status reads and the clone
safety rules are automated (unit + integration), but nothing in the sandbox may
start a clone against a live disk, and the two loop devices the older live pass
used are not present here. **To check on the next live-host pass:** (a) run a
real device-to-device clone on two spare devices and confirm the `--force`
destination write and completion; (b) start a clone on a slow/loop pair, press
**Cancel Clone**, and confirm `kill -0 -<pgid>` fails after the 10 s grace —
i.e. the `sudo ddrescue` child is gone, not just the `sudo` wrapper — and that
the SSE reports `cancelled` (not `completed`); (c) restart the same job and
confirm it resumes from the mapfile kept in `paths::rescue_dir()`; (d) run a
PhotoRec carve as an unprivileged admin and confirm the recovered
`recup_dir.N` is readable by that account (the `chown -R`); (e) point a clone
at a genuinely mounted destination and confirm the in-use guard refuses it by
name, and at an md/LVM member disk (no mountpoint, sysfs `holders` set) and
confirm the same; (f) confirm the destination-smaller-than-source refusal on
two real devices of different sizes.

**2026-10-21 — the Shares config-writing path needs a real host pass.** The
validators, the `/etc/exports` parser/generator and the timeout plumbing are
automated (unit + integration), but every effect below needs real root-owned
files and real daemons: by house rule the integration suite never creates,
toggles or deletes a real share. **To check on the next live-host pass:** (a)
create an SMB share, confirm the appended `[name]` section, `smb.conf` reload
(`systemctl reload smbd`) and reachability from a client; (b) create an NFS
export, disable it from the UI — `/etc/exports` shows the line prefixed with
`# homevault-nfs-disabled:<name>`, `exportfs -v` no longer lists it — then
**reboot**, confirm it is still disabled, and re-enable it (line un-tagged,
`exportfs -ra` restores it); (c) put two exports with the same basename in
`/etc/exports` (`/data/media`, `/backup/media`) and confirm delete/update
answers the new `400` naming both paths instead of touching either; (d) grant
an SFTP share and confirm the `/etc/fstab` bind line is accepted by
`mount -a` and removed on revoke; (e) hammer a slow `systemctl restart smbd`
(e.g. by stopping smbd first) and confirm the 30 s timeout surfaces as a clean
`success:false` message rather than a pending request.

**2026-10-21 — the VPN routine needs a real host pass.** The start-verify
plumbing is unit/integration tested at the pure-verdict level, but a genuine
tunnel needs root interfaces and real daemons (the sandbox has wg/wg-quick
absent and no CAP_NET_ADMIN for `openvpn`'s TUNSETIFF — the 2.6.19
experiments could reach the handshake and beyond only while tunnelled in a
container-less environment). **To check on the next live-host pass:** (a)
WireGuard — provision a device, bring up the phone/laptop client, the peers
list flips to `connected` within 180 s of a handshake, config re-display
(GET `/wireguard/peers/{id}/config`) regenerates the same config, revoke
removes the live peer and the reboot persists the removal; (b) OpenVPN —
upload a real provider profile, verify the start returns within ~1–10 s
**positive** with the tunnel up (`ip addr` shows the profile's tun device),
then a deliberately bad start (wrong password / unreachable endpoint) — the
request fails after the grace naming the log error line, and the daemon is
not left behind (`pgrep -f "--config /etc/openvpn/<name>.ovpn"` empty); (c)
the 409 overwrite guard — attempt to re-upload a running name; (d) confirm
the credentials file `/etc/openvpn/openvpn-<name>.auth` appears and is
deleted at stop.

**2026-10-21 — the app-proxy auth round-trip under a real HTTPS deployment.**
The redirect logic is fully automated (`serve.rs` unit + step 7 of
`tests/app_proxy_integration.rs` — with `TLS_ACTIVE` up the gate answers
`https://…:8443/app-auth?app=…`), but the end-to-end browser round-trip needs
the real listeners: **to check on the next live pass with HTTPS enabled**
(self-signed cert accepted, gate toggled on for a running app): (a) a signed-out
browser hitting the app port → 302 to `https://host:8443/app-auth` → login
screen → after sign-in the `?next` hand-off lands on `/app-auth` → minted cookie
→ back to the app, no loop; (b) the same path re-verified on GFM: the
`hv_app_session` cookie must appear with **no** `Secure` flag in devtools while
`hv_session` carries one; (c) Home Assistant (no compose port, fixed 8123)
once with its proxy auth on.

**2026-10-21 — the netplan apply-verify-rollback path needs a real host pass.**
The verdict (`check_live_state_matches`) is fully unit-tested, but the live
path shells out to `netplan`/`ip` and mutates real interfaces — out of reach
of the Rust suites by house rule (the integration matrix never PUTs a real
config). **To check on the next live-host pass:** (a) through the API, a
static change on a real interface — live address + default route present
right after the request, `netplan apply` reconciled within the grace window;
(b) a DHCP change — a lease converged after the apply; (c) a deliberately
failing apply (invalid gateway) — the rollback restores both the previous
netplan file and the previous live address, the operator session stays up,
and the API answers `success:false` naming what moved. The sudoers-scoped
shapes (`netplan apply`, `cat/chmod/rm -f` on `/etc/netplan/99-homevault-*.yaml`)
also need a non-root service mode check once.

**2026-10-12 — the filesystem half of the user-deletion revoke has no Rust
net** (the database half does, see the 2026-10-12 addendum above).
`PermissionService::revoke_all_user_permissions` really shells out to
`sudo setfacl` on the share path, which CLAUDE.md's "What NOT to test in
Rust" keeps out of the suite: `test_user_delete_revokes_its_direct_share
_permissions_only` deliberately uses share names that do not exist on the
host, so the ACL branch fails cleanly and only the DB cleanup is asserted.
**To check on the next live-host pass:** grant a user a real ACL on a real
share (`getfacl` shows `user:<name>:rwx` + its `default:` twin), delete the
account, and confirm `getfacl` no longer lists it **and** that
`share_permissions` has no row left for the name. Also confirm the intended
non-effect: a **group** grant on the same share still lists the group and
still works for its remaining members.
**Attempted 2026-10-20 and deferred:** the scratch-instance pass ran on a
session with no passwordless sudo (`sudo -n` fails), and every `setfacl`
site is a root wrap — the live step needs the next host with the
sudoers NOPASSWD drop-in; the procedure above is unchanged, complete,
and needs no new code.

**2026-10-12 — the Explorer directory copy/move needs a live re-run** (the SSE
half of the `JobType` fix, see the 2026-10-12 addendum above). The serialization
failure is unit-tested (a `Custom`-job `ProgressEvent` used to fail
`serde_json::to_string`, so the SSE `data:` line came out empty), but the
user-visible consequence — the progress bar animating during a copy, and the
directory listing refreshing once it finishes (`watchJob` resolving `true`) —
has not been observed on a browser since the fix. **To check on the next live
pass:** copy a directory (cross-filesystem, so a tracked job actually starts),
and confirm the progress bar moves, the copy ends on success rather than the
false "failed" it used to report, and the destination listing refreshes. The
Rescue ddrescue/PhotoRec progress display (also `Custom` jobs) deserves the same
check.
**Explorer half closed 2026-10-20 (API-level live, the browser sidebar already
covered by `explorer.spec.ts`'s job tests):** scratch server `:8099`, an admin
session (admins pass `validate_path_access` on any path), a 289 MB / 48-file
tree copied via `POST /api/explorer/copy`. The SSE stream
`GET /api/progress/{job_id}` emitted the intermediate `progress` ticks (0 →
100, ~2 % per 300 ms poll) and closed with **two `done` events** — running →
completed — each tagged `job_type: {"type":"custom","name":"explorer_copy"}`,
i.e. exactly the adjacent-tagging the 2026-10-12 fix restored; no empty
`data:` line anywhere in the capture. Destination verified byte-identical
(48/48 files, aggregate SHA-256 match) and the job reported success, not the
pre-fix false "failed". **The Rescue ddrescue/PhotoRec display stays on this
list** — same `Custom`-job family, but it needs real devices and root.


**2026-10-07 — Quotas removed from this list** (explicit request "implement
[the] task 01"). It was the **last screen** that had neither a live audit nor a
spec run: the spec written on 2026-09-22
(`tests/e2e/quotas/quotas.spec.ts`) is **38/38 green** on a scratch server
(Firefox, isolated `PORT=` + `DATA_DIRECTORY=`) and the screen was audited live
(detail in the 2026-10-07 addendum of `CLAUDE.md` and line 6 of the detail by
screen). **No dead selector fixed** — the spec had been written after the
Appliance migration of 2026-09-12 and already targets the `data-testid`, it passed
as is. Compilation had been acquired since 2026-09-14 (`npm run check`
0 errors after fixing 2 real defects in `Quotas.svelte`, chunk
`Quotas-*` emitted, `cargo build` OK). **Remains open, outside the scope of this
pass:** no integration file of its own — the single endpoint
`GET /transfer-quotas/{type}` remained covered by `transfer_quota.rs` (unit)
+ `explorer_integration.rs` (quota guards). **Closed 2026-10-23 (ROADMAP 23):**
`tests/quotas_integration.rs` was recreated with 15 tests covering the
transfer-quota API itself (admin-only list, self-or-admin single read, counting
periods, window settlement), so the screen now has an integration file of its
own. **Consequence:** none of the 27 screens is without an automated test of at
least one category.

**2026-09-07 — Health removed from this list** (explicit request "implement
the complete tests for Health"): `#[cfg(test)] mod tests` in
`src/health.rs` (13), `tests/health_integration.rs` (10) and
`tests/e2e/health/health.spec.ts` (36 runtime tests, 12 categories,
read-only — **36/36 green** against the live server).
`handle_health_endpoint` was made `pub` (in the
frozen file `src/web/mod.rs`) to allow the integration test to reach a route
— behavior, session check and field names `checks`/
`overall_status`/`status` unchanged. No bug found, the audit of
2026-07-16 still holds. **What is not done:** forcing a check into
`warning`/`unhealthy` state (depends on the host — the aggregate is verified by
consistency with what the host returns).

**2026-09-07 — Desktop/shell removed from this list** (explicit request
"continue Desktop/shell"): `tests/desktop_integration.rs` (3 — **3/3** —
auth guards on `POST /api/server/restart` + `/shutdown`, never with an
admin token because that `spawn`s a real `shutdown -r/-h now`) and
`tests/e2e/desktop/desktop.spec.ts` (35 tests, 12 categories — **35/35
green** on 2026-09-08 against the rebuilt binary ; test 4.4 added with
the standardization of the topbar popovers: the 3 (network / account /
notifications) now share all the card chrome via
`.hv-topbar-panel` ; 2 shell bugs found+fixed:
the dock activity dot never showed for a freshly opened app — non-reactive
`{#if}` condition in `Desktop.svelte` — and Escape
did not close the launcher when the search field had focus —
`stopPropagation` too broad on `.lp-panel` in `HvLaunchpad.svelte`).
The logic/unit is **not applicable**. E2E: rendering of the desktop/dock/topbar pill/clock/user
pill ; the dock opens an app + activity dot + deduplication on
re-click ; windows (close / minimize→dock / maximize→restore) ;
mutually exclusive topbar panels (account menu ↔ notifications, outside click
closes) ; app launcher (opening, search filter,
clicking a tile opens the app + closes the launcher, Escape) ; global
search (Ctrl+K → overlay → result opens the app + closes) ; Power menu
(opening, the Restart option shows a confirmation that is **cancelled** —
`/api/server/*` **never** called) ; responsive (< 480 px hides
date/username), a11y (dock = labeled buttons, no duplicated `id`),
console without errors, edge cases, memory regression. **Never done:**
confirming a restart/shutdown/logout (the former reboots the host, the
latter would end the ongoing session mid-suite).

**2026-09-16 — app launcher redone on the Figma mockup `23:6646`.**
`HvLaunchpad.svelte` is no longer an opaque modal but a darkened full-screen
veil with category chips and a tile grid ; full detail
(contrast fixes, state badge, survival of the `desktop.app_*_info` warnings
as an accessible description, E2E contract preserved) in
the Desktop line of the per-screen table. Category 5 of
`desktop.spec.ts` goes from 4 to 12 tests — 5.5 the chips filter the grid
in place (`aria-pressed`, only one active), 5.6 empty state + `Reset`,
5.7 `fixed` dark translucent veil and content column without surface,
5.8 **composite and measured** label contrast (tile → veil → white
paper, worst case) ≥ 4.5:1, 5.9 44px targets (chip 32px + `::after` 44px,
field 44px in a 56px bar, tile ≥ 44px), 5.10 focus trap
(30 Tab) + Escape returns focus to the dock tile, 5.11 phone — 3-column
grid, chips in `nowrap`/`overflow-x:auto` (`flex: none`, the desktop
growth being cancelled) and **actually overflowing**, the
panel **scrolls internally** (no page scroll, no horizontal scroll)
and the last tile remains reachable, 5.12 six columns at
1440px **and** search bar / chip row / grid at the same width
and same `x` (explicit request: the three modules must never
misalign), **and** the content of the chip row — not just its
box — goes to the right edge of the grid, line by line (request
"not enough categories → enlarge the module": a chip is only as
wide as its label, `flex: 1 1 auto` makes it grow ; the per-line
assertion also holds when the row wraps),
then **the same measurement repeated at 1440×520** — the scrollable container is
the panel and not the grid wrapper, otherwise a scrollbar
that takes width would remove it from the grid alone.
**Two real defects were found while writing
these tests, not by re-reading**: (1) 5.5/5.6 revealed that the `<svelte:window onclick>` of
`Desktop.svelte` closed the launcher on each internal click — `.lp-panel` took
back its stopPropagation `onclick` (without `onkeydown`, which swallowed Escape) ;
(2) the measurement of the `backdrop-filter` showed that the pair
`backdrop-filter` + `-webkit-backdrop-filter` is reduced by the minifier to
the **only** prefixed form, which Firefox ignores → the blur of the mockup
was worth `none`. **Third request of the same day — "if the categories module
is smaller than the width of the 6 columns because not enough categories =
enlarge the category module"**: the box of `.lp-cats` already measured 828px
(5.12 measured it), but a chip is only as wide as its label, so
with few categories the row stopped two thirds of the way and the
module *appeared* narrower than the tiles. `flex: 1 1 auto` on
`.lp-cat` (emitted `flex: auto` by the minifier) shares the free space of
each line among its chips — growth in equal *delta*, so a longer
label remains the wider chip — and not `space-between`, which would have opened
a gap on a short last line. The `≤640px` block takes back `flex: none`
(otherwise a stretched chip would fill the row and the phone's lateral
scrolling would have nothing left to scroll). 5.12 gained the "per line"
assertions and 5.11 the `flexGrow === '0'` check + a row actually
overflowing. Desktop suite:
**73 tests / 15 categories** (`grep -cE '^\s*test\('` = 73 =
`npx playwright test --list` (146 = 73 × 2 projects)). On 2026-09-17, **the desktop
suite re-run in full** against the rebuilt binary: **65 green +
6 skipped + 2 failures**, the two failures both being defects of the **test**,
not of the product, and both fixed then revalidated (cf. below) — the
6 skips being §13 ("no observable background task on this host",
pre-existing). Neither failure is a product regression:
(1) **14.11** compared the raw `class` attribute and hit the Svelte scope
class (fixed: filtering of the `svelte-*` tokens) ; re-run alone, it is
**green** in 1.7 min, its two additional checks included. (2)
**5.6** failed on `browserContext.close: ENOENT … recording41.trace`, a
**teardown** error and not an assertion: two Playwright processes launched
in parallel write into the **same** `test-results/.playwright-artifacts-0/`
(they share the repository root), and one cleans the artifact while the
other closes it. Proof: re-run **alone**, 5.6 is **green**. Rule that follows —
**never launch two Playwright suites in parallel on this repository**,
the second corrupts the teardown of the first (the error looks like a test
failure although the test succeeded).

**2026-09-17 — app headers: the bar carries only [ICON] [TITLE]
(explicit request "verify in all apps that in the windowHeader only
[APP ICON] [WINDOW TITLE] [—] [□] [×] is present, nothing else").**
Registry audit, **recounted by script on `HEAD`**: **17** of the 24 apps
still filled the bar via the props of `WindowHeader`. The counting must
cover **both forms** of passing a snippet — the `actionLabel={…}` attribute
(CloudSync, Downloader, Explorer, Restic) **and** the child `{#snippet …}` block
(the others) ; a `grep` on the attribute form alone misses 13, and a `grep`
that does not bound itself to the `<WindowHeader>…</WindowHeader>` block
over-counts the `{#snippet actions()}` of `SectionCard` (Rescue ×3, Users).
The 17: Chat (Ollama state badge), CloudSync, Dashboard ("Live" badge),
Downloader, Explorer (`extra` + `actionLabel`), Groups, Health, Journal ("auto
refresh" checkbox + 2 buttons), Network, Rescue (tool status row), Restic
(`extra` + `actionLabel`), Settings (save status), Shares, Syncthing, Terminal
(tab row via `center`), Users ("new user" button), VPN. All emptied, and each
control has a decided location:
- `WindowToolbar` (`toolbarRight` / `toolbarLeft` of `PageLayout`) for 10
  of them — Chat, CloudSync, Downloader, Groups, Journal, Network,
  Restic, Shares, Syncthing, VPN ;
- their own file toolbar, already in the page body, for Explorer ;
- the rail footer for `SettingsSaveButton`, which itself carries the save
  state (color + label + sr-only line `settings-save-status`,
  2026-09-23 — the visible pill above and the caption below were
  removed, only a single element remains) ;
- the side rail for the Users "new user" button
  (`data-testid="users-tile-new"`) ;
- the window content for the Terminal tab row (`terminal-tabs`,
  between the header and `terminal-infobar`) ;
- the page body for the Health global status badge (always rendered) and
  for the Rescue tool availability, now a badge **per tool**
  in each tool's card (`data-testid="rescue-tool-badge"`) ;
- **removed** for the Dashboard "Live" badge: decorative indicator
  (the app revalidates itself), not a piece of data.

Not to be confused with the **12 screens that render a `WindowToolbar`**: two
apps that did **not** fill the bar (`Quotas`, `VirtualMachines`) also render one
for their page actions, hence 10 + 2 = 12.

**New test 14.11 "every app header bar carries the icon + title only"**:
it iterates **the 24 app ids** of `OWN_HEADER_APPS` and requires, for each,
exactly one icon tile + one title, **zero** `.hv-winhdr-actions` and
**zero** `.hv-winhdr-center`, and no interactive element
(`button`/`input`/`select`/`textarea`/`a`/`label`/`[role=button]`) in
`.hv-winhdr` — the tab row lives in `.hv-winhdr-wrap`, **outside** the
bar, and the min/max/close trio is a **sibling** of the bar (it belongs to
`WindowFrame`), so both remain allowed. Two more checks close
the blind spot of the first three, which only look for the *slots* and the
controls: **(4)** any **non**-interactive element added in the bar (a
badge, a state dot, a superfluous wrapper) is caught by pinning the
markup shape — apart from the icon tile, `.hv-winhdr` must contain **exactly**
`.hv-winhdr-text` then `.hv-winhdr-title`, in that order ; **(5)** the icon
tile, the only place where an app's markup is free, is pinned to **a
single** glyph (`.hv-winhdr-icon > *` = 1), which catches the "extra badge"
case. **Lesson from the first pass of 14.11**: check (4) compared the **raw**
`class` attribute, but the Svelte compiler adds its scope class
(`svelte-19iy2no`) to any element targeted by the component's `<style>` — and
`WindowHeader.svelte` styles precisely these two classes. The test therefore failed
on **all** the apps, not just the first ; it now filters the
`svelte-*` tokens. **All the assertions of the loop are `expect.soft`**:
a registry guard that stops at the first faulty app hides the 23 others,
whereas the goal is precisely to name **all** the non-compliant bars in
a single pass (the *web-first* assertions in soft mode keep their
auto-retry). Three opening mechanisms
are needed and the test documents why: `hv-open-app` only resolves
in the `apps` registry, which **does not contain** `terminal` or `settings`
(added separately as `terminalApp` / `settingsApp` in
`Desktop.svelte`) — those open via the dock (`dock-<id>`) — and `account`
has **no** launcher tile: it opens via the user pill menu. The test's
timeout is raised to 300 s (24 apps × ~5 s). Two real
findings of this pass: (1) the E2E locators of downloader (15),
vm (2), backup (1), settings (3) and health (2) targeted
`.hv-winhdr-action-btn` / `.hv-winhdr [data-testid=…]`, therefore **empty and
silently green** after the move — rewritten toward the new
`data-testid` (`downloader-add-toggle`, `vm-new`, `restic-add-repo`,
`settings-save-status`) ; (2) `handleOpenAppRequest` cannot open
`terminal` / `settings` (cf. above) — the flaw of the `hv-open-app`
handle that the doc describes as the inter-app app-id passage is real, but
**not fixed** in this pass (outside scope, no caller uses it today) ; to fix
if one day an app must open Settings or the Terminal from its window.

**2026-09-17 (continued) — repair of the cluster of dead E2E selectors.** On
re-running the suites touched by the emptying of the bars, a cluster of
selectors **dead since the "Appliance" migration of 2026-09-12** appeared:
that migration replaced the class hooks with
`data-testid`, and the specs did not follow. Families concerned:
`.ef-file-name`, `.bc-item`, `.modal-input`, `.ef-row-action`,
`.toolbar-btn`, `.toolbar-left`, `.modal-dialog`, `.modal-wide`,
`.preview-editor`, `.preview-text`, `.hv-shares-modal`, `.hv-shares-inp` and
`.hv-modal-dialog`. Two distinct causes, to know before concluding
that a selector is alive: (1) **the Svelte const trap** —
`class={efRowAction}` / `class={toolbarBtn}` put in the DOM the *Tailwind
utility string* that the const contains ; `.ef-row-action` /
`.toolbar-btn` are the **names** of the consts, never class tokens, and
a literal `grep` finds the name, making the selector appear alive ;
(2) **hooks composed at runtime** — `theme/Alert.svelte` renders
`hv-alert hv-alert-{variant}`, and `sidebarTestId="explorer-sidebar"` is a
**prop** that `PageLayout` passes to `WindowSidebar` (a
`grep 'data-testid="explorer-sidebar"'` in `Explorer.svelte` therefore finds
nothing although the attribute is rendered). Only a DOM-level check
decides. `.hv-modal-dialog` was separate: its CSS rule was attached to
**nothing** — the orphan is now documented rather than assumed alive
(and removed 2026-10-19, see below).
**A real hole found by this pass**: the Explorer E2E guard 14.3
("no manual Refresh button") targeted the dead class, therefore matched
**nothing** and was vacuously green — it no longer guarded the decision of
2026-09-08. Rewritten toward
`button[title*="efresh" i], button[title*="ctualiser" i]`, it becomes a
guard again. The 5 "click Refresh to re-list" calls are removed: the
automatic re-list mechanism was **verified before removing the calls**
(`connectWatcher()` opens an SSE `EventSource` on
`GET /api/explorer/watch`, re-lists on `change`, + `visibilitychange`), and
not assumed from the test comment. The root of the modals is
now `[data-slot="dialog-content"]`
(`ui/dialog/dialog-content.svelte:29`) — `journal.spec.ts` and
`users.spec.ts` (4 sites) are repointed ; three were real
assertions already broken, the fourth (`toHaveCount(0)`) passed vacuously.
`Explorer.svelte` gains 17 `data-testid`, `Shares.svelte`
`shares-name-input`.
**Bug of this pass, fixed**: 10 of these attributes had been inserted
in the body of the arrow function of `onclick` (or **before** the `<button`
tag), therefore **outside** the tag — the frontend compilation failed
(`Assigning to rvalue`). Fixed ; a tag-aware scan, run on
the **242 `.svelte`** of the frontend, no longer reports any `data-testid` outside
a tag (the only remaining hit, `SettingsCard.svelte:51`, is in a
documentation comment).
**14.11 hardened**: two checks added, which close the hole reported
by the audit — a **non-interactive** element (badge, state dot, parasitic
wrapper) slipped between the meshes of checks 2 and 3. Now the
elements of the bar **apart from** the icon tile must be exactly
`['hv-winhdr-text', 'hv-winhdr-title']`, and the icon tile must contain
**exactly one** glyph. Prerequisite verified in the source for the 24 apps: the
inline `<svg>` of Dashboard is **one** root (three `<path>` as children),
and Terminal passes `<TerminalIcon>` as `{#snippet icon()}` **child of
`WindowHeader`** — it does not use `PageLayout`, unlike the 23 others.
**Counts (2026-10-19)**: `npx playwright test --list --project=firefox` = **1540 tests /
36 `.spec.ts` files** — that is the reference ; the
`grep -cE '^\s*test\('` documented below gives **1467**, the difference (73)
being the tests **generated by loop**, which that grep does not count. Per suite
(`--list`, recounted 2026-10-18): desktop **84** (`grep -cE '^\s*test\('` agrees — the 74 quoted in the row above is an older recount), users **88**, explorer **64**, vm **45**,
journal **29**.
**Environment limit (groups suite)**: the `beforeAll` of `groups.spec.ts`
creates a throwaway group and fails on this non-root dev machine —
`could not create throwaway group: 400 … sudo: a terminal is required to read
the password`. This is not an application regression but the absence of
passwordless `sudo` for the dev user. In cascade, Playwright
reports **1 failure (1.1) + 62 "did not run"**: the whole file (63 tests)
remains a dead letter, **which includes the 16.1 guard added on 2026-09-23**
(written, counted, but not executed here). To run the suite in full:
NOPASSWD `sudoers` drop-in for `groupadd`/`groupdel` — the same execution
condition as the `beforeAll` of `users.spec` ("absent here", Users line).

**+2026-10-20: `groups/group-identity.spec.ts` (rename + description) is
written to survive that limit instead of dying with the file above: it only
needs a real group for the *editing* half and skips that half with the reason
when the host has none. Green here as **3 passed / 3 skipped** (Firefox,
scratch server :8099): the admin-only gate, an unknown group answering a clean
4xx (never a 500), and the create form's description field run everywhere;
the admin-group refusal (no admin group visible on this host), the description
round-trip and the rename round-trip are the skipped half.** The migration
those skips would have exercised is covered without a host group by the 8 new
`groups_integration.rs` tests (mirror + `resource_group_shares`, the delete
purge so a re-created display name inherits neither a stale description nor
grants nobody made for it, and no phantom mirror row when the group is absent
from the host).

**Execution note (all four 2026-10-20 specs).** Run them in batches of one or
two: chaining `password-change`, `admin-sessions`, `account-expiry`,
`group-access` and `group-identity` in a single Playwright invocation makes the
seeded-admin login fail with `the seeded admin must be available`, because the
auth limiter is **30 sign-ins per minute per IP** (a real anti-abuse guard —
the server log says `Authentication rate limit exceeded for this client IP
127.0.0.1`). Not a regression: each spec alone is green (16/16, 2/2, 4/4,
2 passed + 2 skipped, 3 passed + 3 skipped — Firefox, scratch server :8099).

**2026-09-17 (third pass) — selectors corrupted by the bulk substitution, and
last vacant guards.** The global replacement of the dead classes left
**three syntactically broken selectors**, all of the same mold:
an attribute selector followed by the **rest of the class name** it
replaced. None was visible on re-reading, only at execution (or never, for
the third).
- `login.spec.ts` (new password): `[data-testid="dialog-content"]`
  — the root hook of a modal is `data-slot`, not `data-testid`. The
  scope remains necessary: `login-recovery-submit` exists **twice**
  (`Login.svelte:432` the panel, `:614` the modal), and only the one of
  the modal validates the new password.
- `shares.spec.ts` 4.5: `'[data-slot="dialog-content"]2 button'` — the orphan
  `2` made the selector **invalid** (CSS: unexpected token). Real
  hook: `[data-testid="shares-mode-btn"]` (`Shares.svelte:762-763`).
  Reminder: `segBtn` is a Tailwind **const** (`:490`), therefore never a class
  token — the already documented trap.
- `docker.spec.ts` (Compose editor): `[data-slot="dialog-content"]-close`,
  same invalidation. The live hook of the cross is
  `[data-slot="dialog-close"]` (`ui/dialog/dialog-content.svelte:38`).
- `users.spec.ts` 4.9 clicked `.hv-modal-close`: **class declared but
  applied by no one** (`components.css:353`) — the click timed out. Repointed
  to `[data-slot="dialog-close"]`.
**Two orphan CSS rules** in `lib/styles/theme/components.css`:
`.hv-modal-dialog` (`:323`) and `.hv-modal-close` (`:353`). Their neighbors
`.hv-modal-title` / `.hv-modal-icon` **are** alive (`theme/Modal.svelte`
`:63,65`) — the family is only half orphaned, hence the verification by
reading the component and not by `grep`. **Left in place** (removing from the
shared theme CSS without visual control is not verifiable) ; reported.
**+2026-10-19 — removed after a second DOM-level re-check:** the audit had
already shown both classes matched nothing, so deleting the rules cannot
change any rendered page. The pass removed them together with the four
family members my grep re-confirmed dead (`.hv-modal-overlay`,
`.hv-modal-header`, `.hv-modal-content`, `.hv-modal-footer`) and the three
tokens only the dead rules consumed (`--hv-modal-max-width`,
`--hv-modal-header-padding`, `--hv-modal-close-size`). Kept:
`.hv-modal-title` (rendered by `theme/Modal.svelte:84`) and the
overlay/shadow/backdrop **tokens** — that trio is alive (Drawer, SearchBar,
FirewallCard consume the tokens, not the class).
**Three more vacant guards** converted into real assertions, same family
as the 10 `.hv-winhdr-subtitle` guards: `dashboard.spec.ts` 1.7 and 7.1
(`.db-refresh-btn`) and `settings.spec.ts` 1.2 (`.ah-wrap, .ah-top` — `AppHeader`
disappeared from the entire repository). For Dashboard, the rule is now asserted on
the **accessible name** (`getByRole('button', { name: /refresh|actualiser/i })`),
so a reintroduced Refresh button fails whatever its class ; the
Dashboard today renders **no** button (0 `<button>`, 0 `role="button"`).
**Cross-check of the `data-testid`**: **362** used by the specs against
**454** declared in `frontend/src`. The 7 used-but-non-literal are
all explained: `explorer-sidebar` / `groups-list` / `settings-sidebar`
come from the `sidebarTestId` prop, `docker-nav-item` / `settings-nav-item` from
the `testId` prop of a rail item, `dashboard-live` is the **removed**
badge (deliberate negative assertion) and `dialog-content` was the
misspelled hook, fixed above. No dead `data-testid` remains.
**A third missing hook, this one on the product side** — found on re-running
the security suite after the fix of `openSecurity()`: **2.1 remained
red**, and for an independent reason. `security.spec.ts:302` asserts
`.settings-info-row` in the Antivirus card ; but `AntivirusCard.svelte`
did render this class **at HEAD** (markup + scoped rule), but the migration
to the shared vocabulary lost it: its rows now use
`settingsRow` of `lib/components/settings/vocabulary.ts`, which was **the only
const of the module without its `settings-*` hook** (all the others carry
one — `settings-hint`, `settings-input`, `settings-select`, `settings-btn`…, the
module itself documenting that "the `settings-*` names are the E2E contract, not
of style"). It is therefore not an outdated test selector but a
**hook removed from the product**: the test was right, the vocabulary was wrong.

**First fix — wrong, and it is the mistake to remember.** Adding
`settings-info-row` to `settingsRow` made 2.1 green but **broke
`settings.spec.ts` 10.1** (`toHaveCount(3)` → 4). My justification at the time —
"the 3 lines of `system` that 10.1 counts do not use this const,
verified in the source" — was false: I had read the list of usages of
`settingsRow` truncated by a `head -20`, and **`Settings.svelte:868`** (the line
"update check frequency", a `<select>`, therefore a *command* row) does fall
into the `{#if activeCategory === 'system'}` block opened at
865. The very name of the hook said the error: `settings-info-row` designates a
**read-only** row (label + value), and `settingsRow` is also used for the
command rows.

**Retained fix**: `settingsRow` returns to its form without a hook, and a
dedicated const `settingsInfoRow` (which prefixes `settings-info-row` to `settingsRow`)
carries the hook. `AntivirusCard.svelte` uses it on its **two only** read
rows (`definitions_status`, `last_scan`) — the other rows of the card
(scan path, scheduled scan, quarantine folder) are command rows and keep
`settingsRow`/`settingsRowStacked`. `settings-info-row`
therefore has only one producer, and the 3 literals of `system` remain alone
in their category. Verified: `security.spec.ts` 2.1 **and**
`settings.spec.ts` 10.1 **both green** (2 passed, 27.6s, serial).
To remember: when a card migrates from scoped CSS to the shared vocabulary,
**the E2E hook follows only if it is in the const** — but a shared const
is the right place only if **all** its rows have the semantics of the hook.

**A third bug of the same family, found by E2E and not by grep
(2026-09-17).** After the fix above, `settings.spec.ts` went to
**95/96**: only **23.5** remained red, at `:1807`, `Expected: <= 12`,
`Received: 14`. 23.5 is the only test of the suite that reads a font
size (`parseFloat(cs.fontSize)`) ; it verifies that the section labels are
micro-labels. They rendered at 14px.

Cause: `--hv-text-*` is an **overloaded** prefix — it carries colors
(`--hv-text-primary` = `#1f2733`) *and* sizes (`--hv-text-xs` = `11px`).
Tailwind v4 cannot guess the property targeted by an untyped `var()` and
decides for the color: `text-[var(--hv-text-xs)]` compiles to
`color: var(--hv-text-xs)`. The value not being a valid color, the
declaration is discarded and **no `font-size` is emitted** — the label therefore
inherits the 14px of its parent. The repository convention is the typed
form `text-[length:var(--hv-text-xs)]`, which compiles to `font-size:`
(18 rules `font-size:var(--hv-text-xs` in the 10 CSS chunks).

Scope: three usages, all **sizes** written without the hint — `secLabel` and
`secLabelFirst` of `components/settings/vocabulary.ts`, and the rail header
in `routes/Settings.svelte:657` (without a test hook, therefore invisible to the
suite). Fixed by adding `length:`.

**What this teaches about the documentation.** The guide
`docs/guides/design-system.md` already documented the rule *and* asserted that
"the typographic audit of 2026-09-12 fixed all the existing occurrences".
That was false — three had survived, and it is an E2E test, not a
grep, that found them. The guide is fixed (2026-09-17) with the explanation
of the overloaded prefix and the command that really decides: read the **compiled**
CSS, not the source.

**Method trap, to remember.** My first explanatory comment in
`Settings.svelte` literally quoted `text-[var(--hv-text-xs)]` — and Tailwind
extracts class candidates **also in the comments** of a `.svelte`
(`@import "tailwindcss"` = automatic content detection). Writing
the spelling of the bug in a comment therefore regenerated the dead rule that
had just been removed, and made the source diverge from the CSS actually shipped. The
comment now names only the token.

**Two verification traps encountered in the same pass.** (a) A `grep`
with `\.` at the head cannot match a selector **minified into a list**
(`.a,.text-\[…\]{…}`): several searches wrongly returned "clean" ; the
final checks are done with a literal string (`grep -F`). (b) `grep -c` on
`Settings.svelte` returns 2 for the typed form — the fixed line 657 **plus**
a legitimate pre-existing usage (line 950), not a duplicate of the fix.

**2026-09-07 — Widgets removed from this list** (explicit request "continue
with Widgets"): `tests/widgets_integration.rs` (6 — **6/6**) and
`tests/e2e/widgets/widgets.spec.ts` (22 runtime tests, 9 categories —
**22/22 green**). The logic/unit is
**not applicable** — the only endpoints of the feature (`GET|POST
/api/monitor-layout`, `GET /api/security/active-sessions-count`) are
upserts / DB queries without pure logic. Integration: session guard on
`/monitor-layout` (per-account layout blob: `null` before saving,
round-trip, upsert = 1 row, alice/bob partitioning) ; **admin** guard on
`active-sessions-count` + count semantics (deduplication of a
user with several sessions, expired sessions ignored). E2E: the
catalog window (11 tiles), the active/inactive state of each tile
consistent with the corresponding monitor panel on the desktop, the toggle
that makes the panel appear/disappear + updates the counter of the
footer, the "Reset" button, persistence via `POST /monitor-layout` ;
snapshot+restore of the admin layout in `beforeAll`/`afterAll`.
**Never done:** nothing mutating on the host side (the feature has none).

**2026-09-07 — System journal removed from this list** (explicit request
"continue with System journal"): `#[cfg(test)]` in
`src/handlers/journal.rs` (9 — `priority_text`, `is_app_log_filename`,
`text_log_path` (whitelist + anti-traversal), `strip_target_prefix`,
`parse_app_log_line`, `seconds_to_iso`), `tests/journal_integration.rs`
(11) and `tests/e2e/journal/journal.spec.ts` (28 runtime tests, 12 categories —
**28/28 green 2026-09-07**). **What is
really exercised:** the `app` source (the `homevault.log*` files of
`DATA_DIRECTORY/logs`, which the process owns — no `sudo`) — parsing
of the `tracing` lines, `search` filter, empty state ; `DELETE /clear` which
deletes only `homevault.log*` (never `/var/log` nor journald) ; the
auto-cleanup settings (default on/90 d, round-trip, `interval_days ≥ 1`
clamp). **Never done:** a real `journalctl` / `tail
/var/log/*` (goes through `sudo`) — only the clean fallback `{success:false}`
is verified when `sudo` asks for a password ; the real emptying of the
app logs ; a real triggering of the auto-cleanup scheduler.

**2026-09-07 — Dashboard removed from this list** (explicit request
"start with DASHBOARD"): `#[cfg(test)] mod tests` in `dashboard.rs`
(17 — `format_bytes`, `parse_size`, `calculate_individual_disk_score`,
`calculate_disk_summary`, `calculate_disk_score`),
`tests/dashboard_integration.rs` (10 — **10/10 green** — auth split on
the 15 read endpoints + widget-CRUD + widget-preferences, add/list/reorder/remove
cycle of a widget, per-user preferences upsert + their partitioning
by account) and
`tests/e2e/dashboard/dashboard.spec.ts` (37 `test(` / **39 runtime tests**,
12 categories — **39/39 green 2026-09-07** against the prod binary ;
read-only — the screen has no mutating action ; +2026-09-08: the manual
Refresh button removed (5 s auto-refresh kept), specs aligned). **1 bug
fixed (frozen file, same as
`disk.rs`/`pools.rs`)**: `parse_size` cast the `f64` to `u64` *before*
multiplying by the unit → "9.8G" of `df -h` truncated to "9G"
(under-counting up to ~1 TiB on a size in T), and "465,8G" (locale
comma) parsed to 0. Fixed (`,`→`.`, multiplication in `f64`, cast last) ;
tests `test_parse_size_keeps_the_fraction` /
`_accepts_a_locale_decimal_comma`.

**2026-09-07 — gaps filled in the already partially tested screens**
(explicit request "finish the functions left to test"):
- **Storage / virtual disks**: the handlers `handle_virtual_disks_list`
  / `_delete` / `_scrub` (routes `/api/storage/virtual-disks*`) had
  **no** Rust test. Addition of `handlers/storage.rs::vd_name` (3 unit)
  and of 2 integration tests + the addition of these endpoints to the
  auth matrix loops of `storage_integration.rs` (23 in total). Still no
  listing/destruction of a real virtual disk (real mdadm/ZFS).
- **RAID**: `tests/raid_integration.rs` created (5) — the 5 endpoints
  `/api/storage/raid*` are exercised only up to the auth rejection
  (`require_admin` runs **before** the handler body → `mdadm` never
  reached). Real creation/destruction remains `raid.spec.ts` opt-in.
- **Explorer / Explorer-extra features**: `/roots`, `/save`, `/eject`,
  `/snapshots`, favorites (server-side) and the `?strategy=` parameter on
  `/copy` were without coverage. +11 tests in `explorer_integration.rs`
  (40 in total) + `kind_for_root`/`resolve_dest` unit (5) +
  `CollisionStrategy::parse`/`non_colliding_path` in `browser.rs` (6).
  **2026-09-08 — `explorer.spec.ts` section 15 added** (9 tests): inline
  editor (UI), `/save`, favorites (star ⇄ sidebar ⇄ server), collision
  strategies `/copy`+`/move`, Snapshots menu, `/eject`, Explorer→Shares
  hand-off. **Remains uncovered**: navigation *inside* a real
  ZFS snapshot, a real USB `umount` (host required).
- **Transfer quotas**: `transfer_quota.rs` — `check_quota` (8 unit) plus, since
  2026-10-23, the period/window arithmetic and the reset instant (25 total).
  `applicable_*_quotas`/`record_usage` touch the DB + `id -nG` and are covered
  by `tests/quotas_integration.rs` (15) and the `transfer_quota_repo.rs` units
  (9); `handlers/transfer_quotas.rs` itself still has **0** unit tests — its
  behaviour is asserted through the integration file.
  and remains covered end to end by `explorer_integration.rs` (429 on
  download / blocking on upload) + `users.spec.ts`/`groups.spec.ts`.
- **1 bug fixed (explicit authorization, frozen file)**: `check_quota`
  did `additional_bytes as i64` — a `u64` beyond `i64::MAX`
  became negative and wrongly passed under the limit. Fixed: comparison
  in `u128`. Non-regression test
  `test_check_quota_huge_transfer_saturates_instead_of_overflowing`. See
  `docs/guides/transfer-quotas.md` and the invariant "Explorer / transfers"
  of `CLAUDE.md`.
**2026-09-08 — Settings (core) removed from "without the slightest test"**
(explicit request "finish points 1 and 2"):
- `#[cfg(test)] mod tests` in `src/handlers/settings.rs` (**29** — 2026-10-24, ROADMAP 26.1/26.2: +the appearance model no longer carries `theme`/`density` and an old payload that still sends them still deserializes, +`validate_wallpaper_field` on every accepted client shape and every refused one (the message must start `wallpaper:`), +`validate_accent_color_field` incl. the CSS-injection strings ; was 15 — pure logic:
  `parse_version` / `is_newer_version` (an unreadable version never claims an
  update), `sanitize_hostname` (collapse/trim of the hyphens, fallback
  `homevault`, RFC 1123 limit at 63), `UpdateFrequency` default = `Manual` +
  `as_seconds` (G2 — no automatic GitHub call), appearance defaults,
  `normalize_accent` (2026-09-23 — rewrites the legacy accent `#4a90d9` to
  `#1E62AE` §1.4.3, case-insensitive, any other value is kept),
  `is_check_due` (2026-10-22 — the scheduled-check gate: no interval never
  fires, an opted-in schedule fires exactly at its interval)).
- `#[cfg(test)] mod tests` in `src/settings_persistence.rs` (**15** — 2026-10-24, ROADMAP 26.2: +a legacy on-disk blob carrying `theme`/`density` imports, loads, and is **not** re-exported, +an off-list stored wallpaper is dropped on load (`""`) while the accent survives ; was 7 — `export()`
  masks `smtp_password` (never in clear, `null` in the JSON) without mutating the
  file on disk, `import()` round-trip of the appearance, `apply_loaded_
  settings`: a `None` password (masked export) re-imported **does not replace**
  the configured password).
- `tests/settings_core_integration.rs` (**34** — 2026-11-08: +the `terminal_enabled`
  General toggle persisted/reported by the aggregate and the public settings;
  28 on 2026-10-24, ROADMAP 26.1/26.2;
  the four appearance tests below are the new ones) + Axum router + in-memory SQLite +
  `init_persistence(tempdir)`: **admin** split on all the reads
  (`/settings`, `/appearance`, `/email`, `/notifications`, `/update`,
  `/export`) and writes ; `GET /settings/language` = any
  session ; `GET /settings/public` = without auth and without leak ; round-trip
  PUT→GET of the appearance (accent / density / wallpaper) + reflection in
  the aggregate ; partial update that preserves the other fields ;
  `GET /settings/export` masks `smtp_password` ; export→import ;
  `POST /settings/reset` restores the defaults (accent `#1E62AE`,
  `check_frequency` = `manual`) ; `set-language` accepts en/fr/de/es and
  rejects the unknown ; notification threshold clamped to 100) ; +
  2026-09-23: `PUT /settings/appearance` with the legacy accent `#4a90d9`
  → re-read `#1E62AE` (and reflection in the `GET /settings` aggregate) ; +
  2026-10-22 (ROADMAP 13.1): the update-check gate — `Manual`/`Disabled` make
  zero outbound attempts, `POST /settings/version/check` forces exactly one and
  is cached for the next GET, a non-admin is refused before any call, and a
  `daily` schedule fires once per interval (5 tests, local listener via
  `HOMEVAULT_VERSION_CHECK_URL`) → **18**.
- **Appearance (ROADMAP 26.1/26.2, 2026-10-24)** — the four tests that close the
  gap this row used to describe: (a) `test_appearance_is_readable_by_a_non_admin_session`
  — **the assertion that would have caught 26.1**: a member session gets `200`
  with the admin-set `#123456` + `/wallpapers/hv-11.webp`, the payload is
  **exactly two keys** with no `smtp`/`password`/`webhook`/`check_frequency`
  string, and no-token/bogus-token are still `400`/`401`; (b) a non-admin `PUT`
  is `400 "Admin access required"` and **mutates nothing** (re-read is
  unchanged); (c) a malformed wallpaper (`hv-99`, another host, a non-bundled
  `value`, non-JSON) and a bad accent are `400`s naming the field, and the
  stored wallpaper/accent are untouched; (d) a JSON-encoded `WallpaperConfig`
  is normalised to the bare `/wallpapers/hv-12.webp` URL. The PUT→GET test now
  asserts the two-field shape (`theme`/`density` absent from the response).
  Unit side (`handlers/settings.rs` 25 → **29**, `settings_persistence.rs`
  13 → **15**, `src/i18n.rs` 8 → **9**): the appearance model no longer carries
  `theme`/`density` and an old payload that still sends them deserializes;
  `validate_wallpaper_field` accepts every client shape and refuses every
  off-list one with a `wallpaper:`-prefixed message; `validate_accent_color_field`
  refuses `#fff;background:url(x)`; a legacy on-disk blob with `theme`/`density`
  loads and is not re-exported; an off-list stored wallpaper is dropped on load
  while the accent survives; the two new i18n keys exist in all four locales and
  `settings.density` is gone from all four.
- **Still not covered**: the *runtime* application of the accent
  (`--hv-primary`) — purely front, within the remit of
  `settings.spec.ts` (E2E 4.8 asserts it for a non-admin, written 2026-10-24,
  not yet run) ; `POST /settings/general` (→ `hostnamectl`) is not
  exercised in integration (the shell-out) — `sanitize_hostname` is in
  unit. Since 2026-10-24 (ROADMAP 25) that shell-out — and the `timedatectl`
  one next to it — runs on the blocking pool with a 30 s `run_timed`
  deadline, while `/settings/autostart` gained 4 integration tests driven
  through a `PATH` shim (no host command runs).

**Syncthing** left this list on 2026-09-06: unit tests
`handlers/syncthing.rs` (**18** — `extract_api_key` on the `config.xml`, +
deserialization of the REST responses of syncthing v1.27/1.29 with all the
`#[serde(alias)]`: `deviceID`, `certName`, `myID`, `globalBytes`, `at`,
`clientVersion`… — several of these aliases exist because a missing alias
silently broke the parsing against a real daemon),
`tests/syncthing_integration.rs` (**8**), `tests/e2e/syncthing/syncthing.spec.ts`
(**23**). **What is locked:** the **10** endpoints require an
**admin** session — that was THE flaw of the 2026-07-16 audit: zero
auth check on `restart` (daemon restart), pause/resume
(disturbs any paired device), and reading the IDs/IPs of
all the devices. `pause`/`resume`/`scan` reject a malformed body
(422) before any HTTP call ; when the daemon is off (case of the
current machine), these endpoints return a clean error "Failed to
connect to Syncthing" (never a 500).
**+2026-10-23 (ROADMAP 18.2) — the API key stopped being able to kill the
process.** `<apikey>` is read out of a `config.xml` an administrator can edit by
hand and is interpolated into an outbound header; `HeaderValue::from_str(..).unwrap()`
was on that path, so a control character or a non-ASCII byte **panicked the
request task** (and with `panic = "abort"` the whole appliance). The unit tests
pin the whole matrix (realistic key, indented XML — the most likely real-world
break, because pretty-printing puts a newline in the value — newline/CR/NUL/DEL,
non-ASCII, blank = "no key", and that the message never echoes the key); the
integration tests drive the **real handlers** with a temporary `config.xml` and
assert a named `success:false` error, never a panic. `danger_accept_invalid_certs(true)`
is gone from the client and a source-scan unit test fails if it returns (reqwest
exposes no read-back) — the flag was inert only because the address is loopback
`http`, and it would be a man-in-the-middle the day the address is configurable.
The key read moved to `spawn_blocking`. E2E **4.4** stubs the status error and
requires both the banner and the *absence* of the "install Syncthing" panel
(offering to install an installed daemon is a lie); **4.5** stubs a refused scan
and requires the server reason to reach the screen (the three empty `else {}`
branches were the X.3 entry for this file).
**Never done:** a real
`restart`/`pause`/`resume`/`scan` against a running daemon, nor a real
device/folder cycle (requires a running Syncthing + a paired peer —
like the initial audit, dedicated session) → live read left in
**opt-in** `SYNCTHING_E2E=1`. The 18.2 suites were **written, not executed**
(session instruction); `cargo check --all-targets`, `npm run check` and
`npm run build` pass.
`Syncthing.svelte`
is in *legacy* Svelte mode (no runes) → not touched by the reactivity bug of the
Backup page.

**AI Assistant (Chat)** left this list on 2026-09-06: unit tests
`handlers/chat.rs` (11 — `system_prompt` per language fr/de/es/en
+ English fallback, `session_error_message`, `reset_ai_model`,
`MODEL_CATALOG` + the recommendation scorer that runs against the real
`/proc/meminfo` of the host, deserialization of the Ollama responses),
`tests/chat_integration.rs` (9), `tests/e2e/chat/chat.spec.ts` (22).
**What is locked:** `send` / `send/stream` / `models` / `model`
GET require a **session** (any role — chatting is not
privileged, that was the meaning of the audit fix) ; `models/pull` and
`model` PUT require **admin**. The clean fallback "AI unreachable" (never
a 500) when Ollama is off is tested as a first-class case.
Real conversation in **opt-in** `CHAT_E2E=1` (Ollama running + a model
downloaded). **Verified end to end on 2026-09-06 with a real Ollama
0.33.3 + `gemma3:1b` (CPU inference):** complete suite `CHAT_E2E=1`
**22/22** — incl. §13.1 (real prompt → non-empty streamed response) and §5.1
(the language is indeed transmitted, real response received). In the browser:
**"Online"** badge (green), offline advice hidden, a real prompt
gets a streamed response. `/api/chat/send` → `{"reply":"Pong.",
"model":"gemma3:1b"}`, `/api/chat/send/stream` → SSE `chunk`×N + `done`.
**3 fixes on 2026-09-06 to make Chat functional (explicit
request):**
1. `GET /api/chat/models/recommend` — the **only** Chat endpoint without a
   session check (leaked the host RAM+GPU to any caller). Fixed:
   takes `State`+`headers`, same `get_session` as its siblings. The scorer
   is extracted into `recommend_models(&specs)` (pure, testable).
2. `Chat.svelte` — the status badge displayed a **hardcoded green
   "Online"**, never verified: it stayed green while every
   message failed. Fixed: `onMount` calls `checkOllama()`, the badge
   shows **online** / **no model** / **AI offline** / **check**,
   and the empty state displays actionable advice when Ollama is absent.
3. `installer/install.sh` — Ollama was **never installed**, the Chat
   app was dead on delivery. Addition of `install_ollama` (official
   script, not apt, then `systemctl enable --now ollama`). **No
   model is downloaded automatically** — the choice of the model is up
   to the user (Settings > AI, recommendations based on RAM/GPU).
   `ollama` entry in `dependencies.rs` (`AI` category) + test
   `test_install_sh_installs_ollama_for_the_chat_app` (which also verifies
   the absence of `ollama pull` in `install.sh`).
   **+2026-09-23, strengthened verification against the Ollama Linux doc**
   (`docs.ollama.com/linux`): install *and* Step 4 of verification
   now control the three levels — binary present, `ollama` unit
   active (`systemctl is-active --quiet ollama`, with a 10 s wait
   loop after `enable --now` to let the daemon bind the port) and
   HTTP response on `http://127.0.0.1:11434/`. The
   Rust test requires the three patterns ; a binary installed with a dead
   service now fails the verification instead of displaying a green install
   log with Chat offline.

**Terminal** completed its coverage on 2026-09-06 (it already had 71
E2E tests): addition of the **Rust** tests — `handlers/terminal_pty.rs` (13 —
`is_valid_tab_id` / `resolve_tab_id` / `generate_ticket`, and the cycle of the
single-use connection ticket of the `PtyManager`: issuance → single
consumption, `kill_session` which also purges the pending tickets of the deleted
account, no-ops of `close_tab`/`cleanup_all`/`reap_stale`) and
`tests/terminal_integration.rs` (11 — **admin** auth matrix on
`/api/terminal/ticket` and `/close`, tab id validation → 400, `close_tab`
of an unknown tab = clean success, and a ticket refused with `403` when
`Settings > General > Terminal` is off). **Not tested in Rust:** the spawn
of a real `sudo -u <user> -i` shell, therefore the `MAX_TABS_PER_USER`
ceiling and the kill of the inactivity reaper — that is the role of
`terminal.spec.ts` (real PTY). No bug found.

**Cloud Sync (rclone)** left this list on 2026-09-06: unit tests
`handlers/rclone.rs` (14 on 2026-10-12), `tests/rclone_integration.rs` (17 on
2026-10-12 — the config CRUD moved here from the E2E spec once the path stopped
being hardcoded),
`tests/e2e/cloudsync/cloudsync.spec.ts` (24). **What is locked:**
the 12 endpoints require a session ; all the mutations **and the
reading of the raw config file** (`GET /rclone/config`, which returns
the "obscured" — therefore in clear — credentials of each remote)
require admin ; the anti-injection guards (`config` containing a `[section]`
line), invalid remote name and empty source/destination of
`sync`/`copy` reject cleanly without launching `rclone` nor writing the
file. **Never done:** a real `sync`/`copy`/`mount` `rclone`.
**+2026-10-12:** the real `add remote → list → delete` cycle is now covered in
Rust instead of being left to the opt-in E2E section — the config path stopped
being hardcoded, so the integration suite points `RCLONE_CONFIG` at a tempdir and
drives the real handlers (`rclone_integration.rs`, 14 → 17, incl. the
`.backup.<timestamp>` sibling). A real *`rclone` binary* transfer remains E2E-only
(`CLOUDSYNC_E2E_WRITABLE_CONF=1`, or a server launched with
`RCLONE_CONFIG=/tmp/…/rclone.conf`). No application bug found in the sync paths
(the fixes of the 2026-07-14 audit hold).

**Backup (Restic)** left this list on 2026-09-06: unit tests
`restic_scheduler.rs` (12), `restic.rs` (8),
`tests/restic_integration.rs` (26), `tests/e2e/backup/backup.spec.ts`
(24). **Real conditions:** a real `restic init` + `restic backup` of
`/etc/hostname` in a disposable repository `/tmp/hv-backup-e2e-*`, the list of
snapshots (the snapshot really exists), then deletion of the repository. The
**schedule** and **retention-policy** cycles are 100% DB + `compute_next_
run` — tested end to end in integration without ever launching `restic`.
**Never done:** real `restore`, real `forget`/`prune`, scheduled backup
triggered by the scheduler (30 s loop).
**3 bugs found and fixed (frozen files, explicit request
"fix all the bugs"):**
0. **The Backup page was entirely dead in the browser** —
   permanent loading spinner. `Restic.svelte` is a Svelte 5 *runes mode*
   component but ~35 reactive variables (`resticLoading`,
   `resticAvailable`, `repos`, all the modal flags…) were
   simple `let`: in runes mode a reassignment of `let` does not
   re-render, so `resticLoading = false` in `onMount` had
   no effect. Same class of bug as Shares/Downloader/Rescue;
   audited on 2026-07-14 under Svelte 4, missed by the Svelte 5 migration,
   never detected for lack of tests. Fixed: each reactive `let` →
   `$state(...)`. `backup.spec.ts` fails en masse (24/26 timeouts on
   the spinner) without this fix.
1. `create_backup` did `serde_json::from_str` on **all** the output
   of `restic backup --json`, which is **NDJSON** (`status` lines
   then a `summary` line) → "trailing characters at line 2" → **every
   manual and scheduled backup was reported as failed** whereas the
   snapshot was indeed created. Fixed: `parse_backup_summary` takes the
   last non-`status` JSON line. (Field `total_bytes` → `total_bytes_
   processed` along the way — the real restic name.)
2. `compute_next_run` passed the cron day-of-week field as is to
   the `cron` crate, whose convention differs from crontab: `0` was
   **rejected** (crontab = Sunday) and `1..6` were **shifted by one day**
   (a "every Monday" schedule `* * 1` ran on Sunday). Fixed
   by `normalize_crontab_dow` (crontab `0/7`→Sun, `1..6`→Mon..Sat) ;
   the names `SUN`/`MON`/… already passed correctly.

**Storage** left this list on 2026-09-06: unit tests
`pools.rs` (24), `cache.rs` (13, +2 existing), `encryption.rs` (2),
`tests/storage_integration.rs` (24, +3 in the 2026-09-07 passes) and `tests/e2e/storage/storage.spec.ts`
(30). **2026-09-07 (RAID↔Storage re-scope):** `storage.spec.ts` extended —
test 1.3 now expects **6** tabs ("Virtual disks" added),
the tab nav loop covers the new panel. Run 2026-09-07
against the rebuilt binary: **34 passed / 1 skipped** (LUKS
opt-in cycle). Still **no** test that lists or destroys a real virtual
disk (real mdadm/ZFS required) — only the opt-in §13 of `raid.spec.ts`
covers this path end to end. **What is really exercised:** the whole read-only inventory
(disks/partitions/removable, settings + pools + cache shares, encrypted
volumes) against the real host ; the auth split (read = any
session, write = admin) on the 18 endpoints ; and **each**
mutation path up to the clean rejection (400/401/403, never 500) —
cron/out-of-bounds ranges of the cache settings, invalid cache share
name/mode, too-short passphrase / filesystem / LUKS label
invalid, unknown volume id, deletion without confirmation. **Never
done:** create/destroy/scrub a real ZFS/Btrfs pool, nor format/
unlock/erase a real LUKS volume — all these paths go through
`sudo zpool`/`btrfs`/`cryptsetup`/`mkfs`/`mount`. A real LUKS cycle
(create → verify listed → delete) is planned in `storage.spec.ts`
§13, **disabled by default**: it requires `STORAGE_E2E_LOOP=1` +
`STORAGE_E2E_LOOP_DEV=/dev/loopN` + a passwordless `sudo` (impossible
on the current dev machine — `sudo` there asks for a password). The
unit tests never touch a disk: the invalid-input branches of
`create_pool`/`delete_pool`/`start_scrub` are verified
as returning `Err` **before** reaching a `Command`.
**2 latent bugs found then FIXED on 2026-09-06 (explicit request
of the user to fix all the bugs found):**
1. `PoolService::parse_size` cast the parsed `f64` to `u64` *before*
   multiplying by the unit, so any fractional size of
   `zpool list -H` ("1.81T", "9.06G") was truncated to the integer of
   the unit — a 1.81 TiB pool reported as 1 TiB — and the decimal
   comma of a FR/DE locale ("1,81T" → "181" → 181 TiB) was not
   handled. `disk.rs::parse_size` had **exactly** these two bugs and
   had already been fixed ; `pools.rs` now reproduces the same fix.
   Non-regression tests: `test_parse_size_keeps_the_
   fraction`, `test_parse_size_accepts_a_locale_decimal_comma`.
2. `cache.rs::parse_rsync_stats` split the byte count of
   `rsync --stats` on the spaces before removing the `,` separator —
   under a locale grouping by space ("1 234 567 bytes") it
   recorded only `1`. Fixed: keeps only the ASCII digits
   before the unit word. Test:
   `test_parse_rsync_stats_survives_a_locale_thousands_separator`.

**Downloader** left this list on 2026-09-06: unit tests
`handlers/downloader.rs` (**32** — serde round-trips against fixtures
captured as is on a live `aria2c` 1.37.0: the real RPC
format in camelCase must parse, the JSON returned to the frontend must stay in
snake_case — exactly the bug that the `#[serde(rename(deserialize))]`
fixed ; **+2026-10-23 (ROADMAP 17): 17 new** — the scheme allowlist, the
sub-folder and output-name matrices, the download-root resolver including a
symlink that leaves the root, the event-log redaction and the per-process RPC
secret), `tests/downloader_integration.rs` (**13**) and
`tests/e2e/downloader/downloader.spec.ts` (**46**). **Real condition
really exercised:** a real `aria2c` downloads a file that
the application serves itself (`/wallpapers/hv-01-thumb.webp`, ~25 KB,
without Internet) into a disposable **sub-folder of the download root**
(`<DATA_DIRECTORY>/downloads/hv-dl-e2e-*` — `/tmp` before 2026-10-23), the test
waits for the state
`complete` with the right byte count and the file really present
via `task.files[].path`, then deletes the task (trash button of the UI
_and_ endpoint) and verifies that it disappears from the list. **Never done:**
a real torrent/magnet, nor a remote source requiring Internet.
The integration never triggers the spawn of the `aria2c` daemon (any
endpoint other than `status` would launch it and talk to the daemon of the prod
server on port 6800) — it locks the admin guard on
the 10 endpoints, the body validation (422 before any logic), the
only path without side effect (`GET /status`, which since 2026-10-23 also pins
the reported download root and the absence of the RPC secret), and the refusal
matrix: an invalid link, destination or `out` is answered **without** spawning
aria2, and a child that already exited is reaped as `running: false`.
`downloader.rs` is no longer a _frozen_ page: ROADMAP 17 rewrote its spawn, RPC
and validation paths.

**Explorer (File Explorer)** gained its integration + E2E tests on
2026-09-06 (it only had unit tests `browser.rs`/`config`):
`tests/explorer_integration.rs` (29), `tests/e2e/explorer/explorer.spec.ts`
(50, **49/49 green** against the prod binary), + a `#[cfg(test)]` block
in `src/handlers/explorer.rs` (7). **What is really exercised:**
all the Explorer handlers being simple filesystem calls (no
`sudo`, no service, nothing under `/etc`), the integration drives the real
success paths end to end in a `tempfile::tempdir()` —
mkdir/list/rename/delete, copy+move of a file, text preview, recursive
search, chmod get+set, compress→extract, multipart upload (+ rejection of a
traversing name), byte-exact download (+ folder→400, absent→404), the
transfer quota guard on download (429) and on upload, and the trash
cycle trash→list→restore→purge / empty. The E2E confines all mutation
to a single folder `/tmp/hv-fx-e2e-*` (never `DELETE /api/explorer/trash`
which would empty the trash of a real deployment) ; a disposable non-admin
account proves the `PathGuard` confinement.
**5 robustness bugs found then FIXED on 2026-09-06 (frozen
files, explicit authorization "fix"):**
1. `FileBrowser::restore_trash_item` had no cross-device fallback
   (unlike `trash_item`/`move_item`/`copy_item`) → restoring a
   trashed file from a RAID/pool mount failed with an opaque IO
   error. Addition of the same rename→copy+delete cascade on EXDEV.
2. `find_trash_entry` propagated the `read_dir` error of a nonexistent
   trash folder instead of treating it as `NotFound` → as soon as
   `/home/.homevault-trash` did not exist (normal case), `restore` **and**
   by-id `purge` returned "No such file or directory" and the by-id trash
   was **broken on a real deployment** (verified live).
   Fixed: `ErrorKind::NotFound` → move on to the next folder.
3. `list_directory` failed entirely (`entry?` / `metadata()?`) if a
   single entry posed a problem (broken link, permission, race with a
   concurrent delete) — a dubious file hid the whole folder.
   Fixed: faulty entry ignored, the list continues.
4. No ceiling on the number of entries (a listing of 100k files =
   100k DOM lines). Addition of `MAX_DIR_ENTRIES = 10_000` (after sorting, so
   stable slice) + `truncated` field on `DirectoryListing` + mention
   in the footer (i18n key `explorer.listing_truncated` ×4) +
   filtering of the junk files of other OSes (`.DS_Store`, `Thumbs.db`,
   `._*`, …).
5. Double-click on a folder row unreliable: the 1st click
   selects → inserts the action bar → shifts the list → the 2nd click
   (and the `dblclick` event) misses the row. Fixed in `Explorer.svelte`
   by a double-click detection in JS based on the **identity** of the
   entry (path) and not the screen position (`handleRowClick`), insensitive to
   reflow ; `ondblclick` removed from the rows/tiles.
**No tests added for these 5 fixes nor recompilation** (explicit
request of the user) — to do: `cargo test`, `npm run build`,
then re-run of `explorer.spec.ts` (test 13.2 can then re-include
restore + by-id purge, today bypassed because of bug no. 2).
**+2026-09-23: `cargo test --locked` executed for the first time on this
machine** (Rust toolchain installed via rustup — stable 1.98.1, the machine did
not have one): **green on all the suites, 0 failure** — the `cargo
test` part of this "to do" is therefore covered ; `npm run build` and the
re-run of `explorer.spec.ts` remain.

**2026-09-06 (8 features inspired by the reference design added in 2 batches,
explicit request, without compile or tests):**
Batch 1 — named roots in the sidebar (`GET /api/explorer/roots`) + breadcrumb
with labels ; inline text/code editor (`POST
/api/explorer/save`) ; auto-refresh watcher (8 s poll) ; management of the
copy/move/upload collisions (`?strategy=` : `overwrite`/`keep_both`/`skip`
+ 3-choice prompt).
Batch 2 — favorites pinned in the sidebar (localStorage) ; safe ejection
of USB disks (`POST /api/explorer/eject`, admin) ; "Share this
folder" action which opens the pre-filled Shares app (new store
`explorerShareIntent` + 1 hook in `Desktop.svelte` and `Shares.svelte`) ;
read-only navigation in a past ZFS snapshot (`GET
/api/explorer/snapshots` → `.zfs/snapshot/…`, `snapshotMode`).
**Coverage (caught up):** `explorer_integration.rs` +11 on 2026-09-07
(`/roots`, `/save`, `/eject`, `/snapshots`, favorites, `?strategy=`) ;
`explorer.spec.ts` **section 15 on 2026-09-08** (9 tests, 2 consecutive
green runs) — inline editor (UI), `/save` (writes / overwrites /
rejects > 5 MiB), favorites (toolbar star ⇄ sidebar ⇄
server), collision strategies `skip`/`keep_both`/`overwrite` on
`/copy` and `/move`, Snapshots menu ("no snapshot" outside ZFS),
`/eject` (admin only + refusal of a non-mount-point), "Share this folder"
hand-off → the Shares app opens pre-filled. **Remains not
covered**: navigation *inside* a real ZFS snapshot (`snapshotMode`
with real data), a real USB `umount` — host required.

**RAID** left this list on 2026-09-05: live audit with
disposable loop devices, 9 bugs found and fixed (incl. a total blockage
of the creation — see `CLAUDE.md`), unit tests `disk.rs` (12) and
`raid.spec.ts` added. **2026-09-07:** after the RAID↔Storage re-scope
(RAID = creation wizard only), `raid.spec.ts` was
entirely rewritten (42 `test(`, 43 runtime tests, 14 categories) for the
new wizard ; it never drives a real creation (each test stops before "Confirm
creation", the server validation is exercised by direct `fetch` which 400s before any
`mdadm`/`zpool`) except the opt-in cycle `RAID_E2E_DEVICES`. Run 2026-09-07
against the rebuilt binary: **35 passed / 8 skipped / 0 failure** (the 8 skips require
≥2 free disks on the host to reach step 2/3). One test fixed
after the first run (`10.1` — a POST without a session returns 400, not
401/403: that is the "no token" branch of `require_admin`).

**Rescue** left this list on 2026-09-05: `tests/rescue_integration.rs`
(28) + `tests/e2e/rescue/rescue.spec.ts` (53, never any mutation against a
real device — see the file header) added. Three real bugs found
and fixed (explicit authorization): (1) `get_ddrescue_log` called
`ddrescue --status`, an option that does not exist — the status of an
ongoing clone was therefore always wrong (running=true/0%) ; replaced by a real
parsing of the ddrescue mapfile. (2) 6 streams (ddrescue clone, TestDisk
analyze/rebuild, PhotoRec, mdadm diagnose/repair) displayed no
error on failure — empty `else` branches ; fixed via
`notificationsStore` (same i18n keys already present but never wired).
(3) the shared app header (then `AppHeader.svelte`, since merged into
`WindowHeader.svelte`, ~13 apps): keyboard arrow navigation in
the tabs compared `e.key` to `'IconArrowRight'` etc. instead of
`'ArrowRight'` — totally dead everywhere, fixed.

**Network** and **VPN** left this list on 2026-09-06:
`tests/network_integration.rs` (23) + `tests/e2e/network/network.spec.ts`
(43), `tests/vpn_integration.rs` (22) + `tests/e2e/vpn/vpn.spec.ts` (39),
plus the unit tests `network.rs` (32, +3 `build_resolv_conf` on
2026-09-07) / `vpn.rs` (14). The automated suites do not drive any
real mutation (change an IP, bring down an interface, connect to a WiFi,
bring up a tunnel) — only the access guards and the validation
rejections. **2026-09-07**: the application path of `src/network.rs` was
fixed (mutations `ip`/`dhclient` wrapped in `sudo`,
`/etc/resolv.conf` via `sudo tee` without clobber) and verified live on
a disposable `dummy` interface — see the "Network" invariant of `CLAUDE.md`
and `docs/guides/network.md`.

**Shares/Protocols (Shares)** left this list on 2026-09-06:
`tests/shares_integration.rs` (20) + `tests/e2e/shares/shares.spec.ts`
(51), plus the unit tests of the permission parser (`permissions.rs`
10), of `ShareType` + default configs (`shares.rs` 12) and of the
public SSH key validator (`sftp.rs` 6). The suite never creates / deletes
/ toggles a real share, never starts / stops a real share service,
never grants / revokes a real ACL and never provisions a real SFTP
Linux account nor a bind mount — all these paths go through `sudo` (`tee`/`systemctl`/`exportfs`/`useradd`/
`mount --bind`) or write a root file under `/etc`. The mutations are
exercised only up to confirming a clean rejection (400/401/403, never
a panic/500) on absent/non-admin session or invalid input. E2E suite
launched for real against the prod binary on 2026-09-06: **53/53
green** (Firefox), no real mutation triggered.
**One bug found and fixed (explicit authorization, frozen page):** the
keyboard arrow navigation in the tab bar of `Shares.svelte`
(`onTabKeydown`) compared `e.key` to `'IconArrowRight'`/`'IconArrowLeft'`/…
(icon component names) instead of `'ArrowRight'`/`'ArrowLeft'` —
exactly the bug fixed on 2026-09-05 in the shared app header
(then `AppHeader.svelte`, since merged into `WindowHeader.svelte`), but
here in the page's local copy. The arrow
navigation was totally dead (only Home/End worked) ; fixed and
verified by `shares.spec.ts` test 11.1 (arrows + wrap + Home/End).

**Security suite (Antivirus / Anti-Ransomware / Firewall)** left
this list on 2026-09-06: unit tests (`firewall.rs` 9 — parsing of
`ufw show added` + detection of the mandatory rules ; `antivirus.rs` 6 —
parsing of `clamscan -i` ; `ransomware_watcher.rs` 7 — event
classification + protected path matching ; `ransomware.rs` 3),
`tests/security_integration.rs` (18) and
`tests/e2e/security/security.spec.ts` (30). **Real conditions
really exercised:** a real `clamscan` of an EICAR file in a
disposable folder (detection → quarantine → threat line → resolution),
and real bait files (honeypots) written then deleted on disk
and verified. **Never done:** enabling/disabling the real firewall of the
host or deleting a real `ufw` rule — when `sudo ufw` asks for a
password (common case) `ufw_available()` is `false` and the firewall
card is verified only for the "not installed" state + the access
guards ; the `{confirmed:true}` lock and the 403 on a mandatory rule are
tested live (always without ever enabling) only if a real `ufw`
is reachable.
**A race found and fixed (explicit authorization, frozen page):**
`parse_clamscan_line` read `quarantine_path.exists()` at the exact
moment `clamscan` prints the `FOUND` line, which raced with the `--move`
that follows — on slow storage the threat line was recorded
`action:"left_in_place"` / `quarantine_path:null` whereas the file *is*
indeed moved a handful of milliseconds later (so a later "Resolve + delete"
did not clean the quarantined copy). Fixed: `parse_clamscan_line(&line)`
now only parses path+signature, and a new `resolve_move_status()` re-resolves
the disposition of each threat in a loop *after* `child.wait()`,
when all the moves are finished. Verified by
`security.spec.ts` test 3.1 (real EICAR scan end to end) +
2 unit tests `resolve_move_status`.

**Docker** left this list on 2026-09-05: complete E2E suite
`docker.spec.ts` (49 tests, 14 categories) written and debugged against a
real Docker Engine — no application bug found, only bugs of the
test file (locators badly scoped on the modal which is a DOM sibling
of `.dk-shell`, not a descendant ; close button selector too
broad which intercepted the "Close" button of the hidden Resource Monitor widget ;
`fetch` called in `page.evaluate` before any navigation).

## Live real test 2026-09-07 (Firewall, Shares/SFTP, RAID/Storage, Journal)

The 4 areas exercised end to end against the real host (temporary
NOPASSWD `sudoers` drop-in, 6 loop devices, real `ufw` cycle with
port 22 + the source SSH IP pre-authorized, `homevault-sftp` `sshd`). The whole
environment was restored afterwards (drop-in removed, loops detached,
test accounts/shares/rules deleted, `ufw` left inactive).

**10 bugs fixed (frozen files, explicit authorization), all
re-verified live on the rebuilt binary:**

| # | File(s) | Bug | Impact before fix |
|---|---|---|---|
| 1 | `journal_cleanup_scheduler.rs`, `storage/journal_cleanup_repo.rs`, `handlers/journal.rs` | The auto-cleanup scheduler (default `on`/90 d since G1) runs on the 1st immediate tick ; empty `journal_cleanup_settings` table → `last_cleanup_at=None` → `due=true` ; `set_last_cleanup` was a no-op `UPDATE … WHERE id=1` | `clear_app_logs()` deleted `homevault.log.<day>` just after the appender opened it → **"app" source of the Journal permanently empty, at each restart** ; all the application audit (login/2FA/user-group-share CRUD/Docker/security…) lost. Fixed: `None` branch = set a baseline without cleaning + `set_last_cleanup` UPSERT + `clear_app_logs` **truncates** the day's file instead of deleting it (keeps the inode open by the appender) |
| 2 | `disk.rs::mount_and_persist_raid` | Bare `Command::new("mount")` / `Command::new("blkid")` | mdadm RAID creation: "only the superuser can use mount" → array created but not mounted then tear-down. OK in prod (root), broken outside root, inconsistent with the `sudo mkdir`/`sudo mdadm` right next to it |
| 3 | `disk.rs::create_zfs_pool_from_devices`, `pools.rs::create_zfs_advanced_pool` | `zpool create -f <vdev-type> <name> …` — the pool name must precede the type | **All ZFS creation failed** (`cannot open '<name>': no such device`) for raid1/5/6/10 ; only raid0 passed. Breaks even as root. Also `Command::new("zpool")` without `sudo` |
| 3b | `pools.rs::scan_pools` (`get_mdadm_devices`, `get_mdadm_mount_point`, `get_mdadm_size`, `get_zfs_devices`, `get_zfs_mount_point`) | Storage "Virtual disks" view: wrong `device_count` (mdadm 2 disks → 1 ; ZFS mirror 2 disks → 5), `mount_point` always `null` (mdadm: match on the symlink not `/dev/md127` ; ZFS: invalid `zpool get mountpoint` → `zfs get` was needed), `used_bytes` mdadm = fabricated `size/2`, `size_bytes` mdadm under-counted ×1024 (KiB read as bytes) | Misleading metadata (never blocking). Fixed: parse of the `mdadm --detail` table, `canonicalize`, `zfs get`, real `df`, `blocks*1024` |
| 4 | `disk.rs::delete_raid` | Bare `Command::new("umount")` | Destruction of an mdadm virtual disk: `umount` fails silently → `mdadm --stop` "device busy" → **delete aborted, array + `/etc/fstab` + `mdadm.conf` left in place** |
| 5 | `shares.rs::set_share_enabled` (SMB) | End of section located via `after_section.find("\n[")` — `None` for the **last** section of `smb.conf` | The freshly created SMB share (always added at the end of the file) **can never be disabled** as long as another is not added after it. Lying `"Share disabled"` response. Breaks even as root |
| 6 | `shares.rs` (create/update/delete `_sftp`/`_ftp`/`_webdav`_share, `save_*_config`) | Bare `fs::write` / `fs::create_dir_all` to `/etc/homevault/*.json` (root dir) | Creation of an SFTP/FTP/WebDAV share: `Permission denied` outside root ; inconsistent with `write_file_as_root` everywhere else. Fixed + new helper `mkdir_as_root` |
| 7 | `acl.rs` (0 `setfacl` sites) | Bare `Command::new("setfacl")` | Any share ACL grant/revoke: `EPERM` outside root if the folder does not belong to the process |
| 8 | `acl.rs::remove_acl` | Removes the access entry but not the corresponding **default** ACL | A revoked user remained granted on any file created afterwards in the share (`default:user:X:rwx` persisted) |
| 9 | `handlers/shares.rs::handle_sftp_key_set` | No check that the `username` is a HomeVault user | `PUT /api/shares/sftp/users/key` with any name → real `useradd` → orphaned Linux accounts on a typo. Fixed via `state.auth.user_exists()` |
| 10 | `firewall.rs::get_status()` | Greps the English string `Status: active` | On a non-English locale host (`fr_BE` → `ufw` displays `État : actif`, and `LANGUAGE` overrides even `LC_ALL=C`) → `enabled` **always `false`**: the Firewall card shows "disabled" while `ufw` is active. Fixed: `ufw_command()` helper which forces `LC_ALL=C`/`LANGUAGE=` (kept by the default `env_keep` of `sudo`) |

**Verified OK live (no bug):** firewall `{confirmed:true}` guard,
403 on deletion of a mandatory rule, rules visible with `ufw`
disabled, `ensure_mandatory_rules` (SSH survives the `enable`) ;
`rebuild_jails` (bind-mount + `fstab`), `purge_user` on deletion of a
HomeVault user, default ACL (`-d`/`-m` order) ; native Btrfs complete
cycle ; `sudo journalctl`/`tail` + unit/priority/search filters of the
Journal ; rejection of traversal `source=../../etc/passwd`.

**Still not covered by automated test** (requires the real host — remains
E2E/manual): a real `ufw enable` (possible network restart), a
real SMB/NFS/SFTP share created, a real `mdadm`/`zpool create`. The
integration suites still stop at the "clean rejection".

## 2026-09-07 (continued) — de/es i18n + rest of the host tests

**de/es i18n (done).** `src/i18n/de.toml` and `es.toml` had the same
defect as `fr` (whole sections written without diacritics). Restoration
of the accents **values only** (keys + `%placeholders%` verified
intact, `tomllib` + `cargo build --release` clean): ~350 lines each,
via curated whole-word maps (`fix_de.py` / `fix_es.py`, scratch dir).
`de`: digraphs `ae/oe/ue/ss` → `ä/ö/ü/ß` (false positives `aktuell`,
`daemon`, `neue`, `manuell`, `quell`, `steuer`, `virtuelle`, … excluded).
`es`: singular `-ción/-sión`, `éxito/código/tamaño/batería/contraseña/…`,
a few future tenses, `¿?` on the obvious questions ; plurals `-ciones` and
`solo` left as is (correct). The embedded `de`/`es`/`fr` block of the
frontend (`i18n.ts`) was already correctly accented — untouched.

**2nd pass fr + es ("do another tour of the project").** The 1st fr pass had
wrongly classified `[settings]` `[shares]` `[users]` `[groups]` `[docker]`
(residuals), `[journal]` `[common]` `[cache]` `[account]` as "already
accented" — they were in fact largely bare. Fixed via `fix_fr.py`
(~155 lines, whole-word ; all the ambiguous verb/present forms —
`utilise`, `gere`, `affiche`, `chiffre`, `complete`, `limite`, `detecte`,
`synchronise`, `verifie`, `deploye`, `genere`, `execute`… — **excluded from
the map**). 5 imperative-vs-participle mis-accents ("Installe/Supprime/
Démarre…") + `À propos`/`À utiliser` + 2 `_synced` fixed by hand
after reviewing the diff. `es`: `máscara`, `estática`, `público`, futures
`aparecerán/aplicarán/formarán/moverán/perderán` (~11 lines).

**Complete linguistic verification + removal of the orphans
(2026-09-08).** Key parity now **identical** in the 4
languages (`en:common.browse_button` → `common.browse`) ; `%placeholders%`
consistent everywhere (0 discrepancy) ; no empty value ; no English leak.
Last accent residues fixed (26 lines: fr 9, de 7 "bare
vowel" — `konnte`/`wurde` = correct preterite, untouched —, es 9).
**825 orphan keys removed** from the 4 files (3048 → **2223**) —
analysis: all the enumerated i18n lookups (static + fixed list of
dynamic prefixes + `docker.${actionKey}` with 6 values), 0/825 cited
bounded in the source, 25/25 clean spot-checks, `cargo build` +
`svelte-check` 0 errors, E2E `dashboard`/`health`/`widgets`/`settings`/
`explorer`/`network` green.

**2026-09-14 — 2nd pass, orphans of the header standardization.**
19 more keys removed from the 4 files (2522 → **2503**): they are the
subtitles removed from the `WindowHeader`s — `docker.containers_summary`,
`users.summary`, `quotas.subtitle`, `groups.summary`, `shares.summary`,
`account.subtitle_{admin,user}`, `dashboard.overview_subtitle`,
`rclone.subtitle`, `raid.manager.header_subtitle`, `network.subtitle`,
`chat.subtitle_default`, `chat.model_via`, `vpn.subtitle`,
`downloader.subtitle`, `syncthing.subtitle`, `widgets.subtitle`,
`rescue.subtitle`, `journal.subtitle`. Removal **by (section, key)** and
not by leaf name: six other `subtitle` are alive and were kept after
verification one by one (`settings.` = rail label +
nav `aria-label`, `restic.` = `tabsLabel`, `login.` = login page,
`antivirus.`/`ransomware.`/`firewall.` = `SettingsCard` subtitles).
`docker.${actionKey}` re-checked — it only produces the 6
`{container,stack}_{started,stopped,restarted}`, `containers_summary` is
therefore safe. Verified afterwards by script: 2503 keys in each of the 4
languages, **identical** key sets, identical `%placeholders%` sets
(0 discrepancy), 19/19 keys absent everywhere, 6/6 kept keys present
everywhere, no emptied section. **Nothing was compiled or re-run.**

The test counters of this document do not move: this pass only touches
`src/i18n/*.toml` and `size=` attributes of icon snippets.

**Remains**: native review of `es` (verb tenses in long sentences) ;
a few scattered `a`↔`à` in `fr`.

**Real host tests (temporary `/etc/sudoers.d` NOPASSWD drop-in, removed
afterwards ; 4 loop devices + a disposable RAID1 ; environment restored).**

- **Rescue — DONE (never exercised live before). 6 bugs fixed** (all
  frozen `src/rescue/*`, re-verified live): ddrescue/testdisk/photorec
  never `sudo`-wrapped (only mdadm was) → on non-root everything failed
  silently (`success:true` + empty partitions) ; `list_block_devices`
  parsed `lsblk -o` positionally (MODEL has spaces → wrong model/
  transport/removable) + private `parse_size` with the
  cast-before-multiplication bug + locale comma ("465,8G" → `size:0`) —
  rewritten as `lsblk -P -b` ; `mdadm_diagnose` read the header
  line `/dev/md127:` instead of the member table ; health of a degraded
  mirror (`"clean, degraded"`) returned `healthy` (order of the `contains`) ;
  `parse_testdisk_output` offsets wrong for TestDisk 7.1 `/list` (never
  executed because no sudo) → rewritten + 3 unit tests ;
  `testdisk_rebuild` never worked (stdin script ignored, device
  missing) **and could now block the worker** (TestDisk 7.1
  `/cmd` `write` waits for a Y/N that stdin cannot give) → reduced to
  `analyze,search` + `stdin(null)` + 300 s timeout + `kill_on_drop`. See
  the "Storage / RAID / encryption" invariant of `CLAUDE.md` and
  `docs/guides/rescue.md`.
- **Storage / LUKS — DONE. Complete cycle** create → write a file →
  `/lock` → `/unlock` (wrong passphrase rejected, correct one brings it back, data
  intact) → `DELETE {confirm:true}` against a real `/dev/loopN` +
  `cryptsetup`. The lifecycle (sudo-wrap + `write_file_as_root` of
  2026-07-17) holds. **2 bugs fixed** (frozen): `handle_encryption_
  create` did **not** validate `req.device` → an admin could pass
  `/dev/sda` to `cryptsetup luksFormat` (now guarded by
  `get_available_devices` `!in_use`) ; `encryption::erase` left
  the LUKS header → device stuck `in_use` forever (addition of a
  best-effort `wipefs -a`). Invariant under "Storage / RAID / encryption"
  (`CLAUDE.md`) and `docs/guides/storage.md`.
- **Network — reads re-verified live + `sudo` bugs of the application path
  FIXED** (frozen `src/network.rs`, on "fix what is needed").
  Reads OK (`/interfaces`, `/diagnostics`, `/summary`, `/services`,
  `/public-ip`, `/wifi/status`, `POST /wifi/scan`), rejection of invalid body
  OK. Fixed: helper `ip_sudo(program)` — each *mutation* `ip`/`dhclient`
  (`addr add/flush`, `route replace`, `link set up/down`,
  `dhclient -6`) goes through `sudo` (the read `ip … show`/`ping`/`ethtool`
  remain bare) ; `/etc/resolv.conf` + the toggles
  `/proc/sys/net/ipv6/conf/<iface>/*` written via `sudo tee`
  (`write_as_root`) instead of a silent `std::fs::write` ; DNS no longer
  rewrites anything but the `nameserver` lines (`build_resolv_conf`, pure,
  3 unit tests — `search`/`options`/comments kept).
  **Real mutations still NOT exercised** (host over SSH) and the
  **persistence** (netplan / `/etc/network/interfaces` — `ip addr add` is
  runtime-only) remains a deliberate follow-up (backend choice + console
  session). Invariant under "Network" (`CLAUDE.md`) and
  `docs/guides/network.md`.

**2026-10-16 (four desktop/plumbing features).** Four features landed in one
pass, each with its own tests and doc updates:

1. **Single settings catalog** (`frontend/src/lib/stores/settingsCatalog.ts`)
   now feeds the Settings rail, the content header and the global ⌘K index
   (`settingsSearchItems`). E2E `desktop.spec.ts` 6.5–6.7 (keyword search,
   category activation, wallpaper shortcut). Doc: `docs/guides/settings.md`.
2. **Live usage by app** — `components/dashboard/AppUsageBreakdown.svelte` +
   `charts/AreaChart.svelte` + `lib/utils/appColor.ts`, embedded in the
   Dashboard. E2E `dashboard.spec.ts` 2.6/2.7 and 3.4/3.5 (menu freeze, menu
   contents, no mutation on open). Doc: `docs/guides/dashboard.md`.
3. **Per-member resource access** — migration `041_resource_shares.sql`,
   `storage/resource_share_repo.rs`, `handlers/resource_shares.rs`
   (`/shares/access` + `/shares/access/mine`, server-side filter helpers +
   WS whitelist). Tests: 5 unit + 10 integration
   (`tests/resource_shares_integration.rs`) + E2E `users.spec.ts` 17.1–17.4.
   Docs: `docs/guides/users.md`, `docs/API.md`.
   **+2026-10-20 — group-level grants** (migration
   `051_resource_group_shares.sql`): `resource_group_shares` keyed by group
   display name, `/shares/access/groups` (GET/POST/DELETE, admin-only),
   `inherited` on `/shares/access/mine`, and `effective_visible_resources()`
   resolving direct ∪ group grants from **live** OS membership with
   `merge_strongest` (strongest level wins, unknown levels ignored). Tests: 3
   new unit (`merge_strongest` — stronger level wins both ways, union sorted,
   corrupt levels fail closed) → 8 in `resource_share_repo`, 5 new integration
   (`resource_shares_integration.rs` 10 → **15**: group CRUD + idempotent
   upsert + "no such group grant", all three group routes admin-only, unknown
   kind refused, the union end-to-end on the repository — strongest wins,
   revoking the group row demotes nobody holding a direct row, another group
   contributes nothing — and `revoke_all_for_group` across all three kinds),
   plus E2E `tests/e2e/users/group-access.spec.ts` (**2 passed / 2 skipped**:
   the grant CRUD + admin-only rule run everywhere; the inheritance half needs
   `groupadd` over NOPASSWD sudo and is skipped with that reason, the same
   documented host limitation as `users.spec.ts`'s `beforeAll`). Group
   deletion now purges the group's grants. Docs: `docs/guides/users.md`,
   `docs/API.md`, `docs/BACKEND.md`.
4. **Web shortcuts + Edit Widgets** — SSRF-guarded
   `POST /shortcuts/fetch-metadata`, `stores/shortcuts.ts`,
   `DesktopShortcuts.svelte`; `routes/EditWidgets.svelte` +
   `stores/widgetLayout.ts` (drag & drop, max 3 featured). Tests: 7 unit + 3
   integration (`tests/shortcuts_integration.rs`) + E2E `shortcuts.spec.ts`
   and `edit-widgets.spec.ts`. Docs: `docs/guides/shortcuts.md`,
   `docs/guides/widgets.md`, `docs/INDEX.md`, `docs/API.md`.

`cargo test --lib` = **704** passing (was 689); the new guide is indexed by a
`FeatureDoc` (`shortcuts`) so `test_every_guide_has_a_feature_doc` stays green.

## 2026-10-17 — notification center, storage health, app-auth, client contract

Four areas, each with its own tests and docs:

1. **Persistent notification center + What's New.** Unit: `release_notes`
   (3), `api_contract` (5), `middleware::contract` (2). Integration:
   `tests/whats_new_integration.rs` (4 — auth guards, per-user `seen`,
   idempotence) and `tests/api_contract_integration.rs` (2 — unauthenticated
   `/api/meta` shape). E2E: `tests/e2e/desktop/desktop.spec.ts` 16.1–16.4.
   Docs: `docs/guides/notifications.md`, `docs/API.md`.
2. **Storage health — scrub status + foreign-pool import.** Unit:
   `handlers/storage.rs::scrub_in_progress` (2) and `pools::tests`
   `parse_zpool_import` / `parse_btrfs_show` / `import_pool` (8). Integration:
   `tests/storage_integration.rs` 28 (was 24) — the two new read/write
   endpoints join the auth-split tables, plus dedicated unknown-name and
   unsupported-type tests. E2E: `tests/e2e/storage/storage.spec.ts` 14.1–14.5.
   Docs: `docs/guides/storage.md`, `docs/API.md`.
3. **App-proxy auth hardening.** Unit: `app_proxy::app_auth` (5),
   `app_proxy::cookie` (3). Integration: `tests/app_proxy_integration.rs`
   (1 end-to-end flow asserting the API session is rejected, the app session
   is accepted, and both cookies are stripped upstream). Docs:
   `docs/guides/docker.md`, `docs/ARCHITECTURE.md`, `CLAUDE.md`.
   **+2026-10-21 (HTTPS production pass):** unit `serve.rs` +4
   (`app_auth_location` — plain-HTTP main entry, TLS entry follow-through,
   custom HTTPS port, query encoding), `app_auth` +1
   (`test_mint_is_capped_and_keeps_the_newest_sessions` — 512-entry cap with
   soonest-expiring eviction). Integration step 7: with `TLS_ACTIVE` set the
   gate redirects to `https://…:8443/app-auth?app=…` and the challenge carries
   `Cache-Control: no-store`; the flag is reset before any assertion. The
   login screen honours the flow's `next` parameter (same-origin paths only).
   `app_proxy/*` unit count: 2 → **17** (**28** on 2026-10-23, ROADMAP 6). Docs:
   `docs/guides/apps-center.md` (rewritten), `docs/API.md`,
   `docs/ARCHITECTURE.md`, `docs/guides/docker.md`, `CHANGELOG.md`, `ROADMAP.md`.
4. **Versioned client contract + DX.** Covered by (1)'s unit/integration
   tests; `src/mdns.rs` gains a `mdns_hostname` unit test. Tooling:
   `scripts/dev-instance.sh`, `playwright.vm.config.ts` +
   `tests/vm/smoke.vm.test.ts` (opt-in real-VM suite). Docs:
   `docs/guides/development.md`, `docs/ARCHITECTURE.md`.

`cargo test --lib` = **734** passing (was 704). The new
`docs/guides/development.md` is registered as developer-only in
`rag_indexers.rs`, so `test_every_guide_has_a_feature_doc` stays green.

## 2026-10-19 — the "add a web shortcut" control moves into the launcher

User request: the add tile must not sit on the desktop; it belongs to the
launcher, **last**, after every app. Frontend-only change (no Rust touched
except the RAG `FeatureDoc` text for `shortcuts` / `desktop`).

- `HvLaunchpad.svelte` gained `onAddShortcut` / `canAddShortcut` and renders
  the add tile as the grid's last cell (`launcher-shortcut-add`);
  `DesktopShortcuts.svelte` lost both the add tile and the dialog (it now
  renders nothing at all when the set is empty); the dialog lives in the new
  `ShortcutAddDialog.svelte`, mounted by `Desktop.svelte`.
- `tests/e2e/shortcuts/shortcuts.spec.ts` re-targeted, **7 → 8** tests
  (`grep -cE '^\s*test\('` and `npx playwright test --list` agree): `1.1` the
  add control is not on the desktop, `1.2` it is the grid's last cell and is
  withheld while a query narrows the grid, `1.3` the dialog opens from it.
  **Two of the old tests had never been able to pass** — `2.3` and `3.2`
  seeded localStorage with `evaluate` + `reload`, but `seedBrowser`'s
  `addInitScript` clears every `hv-shortcuts:*` key on *each* document load,
  so the seed was wiped by the reload meant to pick it up; both now seed
  through `gotoDesktop(page, tokens, shortcut)` (an init script registered
  after the clearing one), and `2.3` reveals the remove button through its
  `focus-within` path instead of clicking an element that is `display: none`.
- `desktop.spec.ts` `5.14` (A→Z) excludes the add tile from the alphabetical
  run — it is a control, not an app — and its filtered half now reuses the
  same selector. No test count change there.
- `shortcuts.open` / `shortcuts.remove` placeholders fixed to `%title%` in all
  four locales (the translator interpolates `%name%`, not `{name}`).

Verified: `npm run check` (0 errors), `npm run build`, `cargo test --lib`
(**854** passing) and the shortcuts spec green on **Firefox and Chromium**
(8/8 each) plus `desktop.spec.ts` category 5 green (Firefox). The E2E ran
against the Vite dev server on `:5199` proxied to the seeded `:8080` backend,
because the running binary embeds the previous `static/dist` build.

## 2026-10-19 — RAID: free-disk detection was inverted; server-side device gate added

Production-readiness audit request on the RAID creation wizard found **one
blocking bug + one data-safety gap**, both fixed in the same change:

- `disk.rs::get_available_devices` computed `in_use = disk.role != "Available"`
  — but `detect_disk_role` can only produce `System | Data | RAID | ZFS Pool |
  Btrfs | Unassigned`, so **every** device returned `inUse: true` and the
  wizard's step 2 (device grid) was permanently empty: **creating a virtual
  disk from the UI was impossible**. Proven live with a compiled probe against
  this machine's real disks (`total=2 inUse=2`). The verdict is now
  facts-based: `gather_device_facts` runs one tree-level `lsblk -P` pass
  (children = partitions/LUKS/LVM disqualify, mount points, filesystem
  signatures) plus membership probes (`sudo mdadm --examine`, `zpool status`,
  `btrfs filesystem show`), and the pure `facts_indicate_in_use` decides
  (unit-tested). Along the way the system-disk exclusion was fixed: `df /`
  reports the **partition** (`/dev/nvme0n1p2`), and a plain trailing-digit
  trim never matched it against the whole disk `nvme0n1` — replaced by the
  symmetric `same_device` (`sda1`/`sda`, `nvme0n1p2`/`nvme0n1`, lookalikes
  `sdaa`/`nvme1n1` rejected).
- `handle_raid_create` barely checked device *existence*; the only "free"
  guard lived in the (buggy) frontend. An admin request built by hand could
  `mdadm --create --run` straight over a mounted, partitioned, or
  filesystem-carrying disk — or the system disk — with zero confirmation
  (`--run` skips mdadm's only interactive prompt). `validate_pool_devices`
  now runs the same facts gate **server-side**, before the mdadm / zpool /
  Btrfs dispatch; non-free or system devices get a clean 400 naming the
  reason (`it has 1 child device…`, `it is mounted at '…'`, …).
- `raid.spec.ts` — **the blind spot that hid the bug**: every disk-step test
  (2.3/2.4/3.2/4.2/5.1/5.2/8.1) skipped silently when the host offered no
  *selectable* rows, which — because of the bug — was always. They now go
  through `requireFreeDisks`: plain hosts still skip, but an opt-in
  `RAID_E2E_DEVICES` run **fails** if the provided devices don't turn up as
  selectable step-2 rows. Added **13.2** (provided devices must appear as
  selectable rows in the UI) and **13.3** (while an array exists, its devices
  are reported `inUse: true` in the device list and a second create on them
  is refused 400 by the server-side gate). Count 43 `test(` / 45 runtime.
- `disk.rs` tests 12 → **17** (`facts_indicate_in_use` all-clear + per-signal,
  `same_device`/`is_same_disk` cases, `in_use_reason`); existing
  create_raid validation tests untouched (the gate runs after them, exercised
  live via the E2E opt-in cycle since it needs real devices).

Verified: `cargo test --lib disk` 17/17 and `--lib pools` 33/33,
`cargo test --test raid_integration` 5/5, `npm run check` 0 errors and
`npm run build`. The device-gate behavior was verified live on real disks
via probes (system disk excluded from the list; a disk with a mounted
partition reported in use with the right reason, and refused by
`validate_pool_devices`). The full UI walk of steps 2–3 requires opt-in loop
devices (`RAID_E2E_DEVICES`) — same opt-in contract as 13.1.

## 2026-10-24 — Web shortcuts: the 512 KB cap bounds the read; a redirect chain is walked hop by hop

ROADMAP 27.1/27.2, the last P1. Two server-side defects, no UI change needed:

- **27.1 — the cap bounded the parser, not the memory.** `read_capped` awaited
  `Response::bytes()` and sliced afterwards, so a multi-GB response was fully
  materialised before the truncation; the 10 s timeout was the only real bound.
  The reader is now streaming (`read_capped_stream` over
  `Response::bytes_stream()`): it stops pulling at `MAX_BODY_BYTES`, cuts a
  chunk that crosses the cap to its prefix, and drops the rest of the body.
- **27.2 — one hop followed, and a rebinding window.** The second redirect's
  empty body was parsed as the page (metadata silently empty on every
  short-link/CDN chain), and `resolve_public` validated addresses that reqwest
  re-resolved at connect time. `fetch_page` now walks up to `MAX_REDIRECTS` (5)
  hops, re-validating each target (scheme, host, every resolved address) before
  dialling it, and the shared client resolves through `PublicOnlyResolver`
  (`reqwest::dns::Resolve`), so the address handed to the connector was
  validated in the same resolution. Only `301/302/303/307/308` are followed;
  a longer chain is a named `400`. The timeout is one chain-wide deadline, and
  one process-wide client replaces the per-request one.

Counts: `src/handlers/shortcuts.rs` **7 → 19** unit — the decisive new
assertions are `test_read_capped_stops_reading_at_the_cap` (the second chunk is
never polled once the cap is reached — the old code would have drained it),
`test_redirect_target_handles_relative_protocol_relative_and_cross_host`,
`test_a_whole_redirect_chain_is_walked_hop_by_hop` (the second hop is where the
old code stopped), `test_redirect_target_refuses_a_bad_scheme_or_a_missing_location`
(a hop must obey the entry URL's allow-list) and
`test_public_only_resolver_refuses_private_literals` (the connect-time half of
the guard). `tests/shortcuts_integration.rs` **3 → 4** —
`test_fetch_metadata_rejects_every_private_literal_form` covers v4-mapped,
unspecified, link-local, unique-local, multicast and the two RFC1918 ranges
(the addresses a malicious `Location` aims at). `shortcuts.spec.ts` **8 → 10**
`test(` — §2.4 (a refused chain: server sentence in the `role="alert"`, typed
URL kept, no tile pinned) and §2.5 (a timeout: submit re-enabled, field
editable), both against a **mocked** route.

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib` (unit target compiles) and
`cargo check --test shortcuts_integration`. `npm run check` / `npm run build`
and the Playwright suites were not run. No test performs a real outbound
request: a loopback test server would be refused by the guard under test, so
the chain and the stream are driven as pure functions (documented in
`docs/guides/shortcuts.md` → *Tests*).

## 2026-10-24 — File Explorer: blocking work off the runtime, and a deadline on every subprocess

ROADMAP X.1 (closed) + X.2 for `handlers/explorer.rs` and `src/browser.rs`.
Nothing here is a new behaviour a user can ask for: the screen answered `200`
before and after. What changed is what happens when a disk, a mount or an
archive tool stops answering.

- **X.1** — the disk scan (`/proc/mounts`, a `stat` per mount, one `lsblk` pass,
  one `df -B1` per mount) became `collect_disks()` behind `spawn_blocking`; the
  path stats of download/upload/copy/move/save/roots/thumbnail/ticket handlers
  go through `tokio::fs` (`path_exists`/`path_is_file`/`path_is_dir`);
  `resolve_dest`, the four `fsync_parent_dir` durability calls and the
  `cleanup_created_paths` sweep all run on the blocking pool; the upload-session
  prune releases its lock before an async unlink.
- **X.2** — every `.output()` in the two modules became `run_timed`:
  10 s for `lsblk`/`df`/`df -T`/`chmod`/`which`, 30 min (`ARCHIVE_TIMEOUT`,
  added to `src/utils/process.rs`) for `tar`/`unzip`/`7z`/`gunzip`, 30 s for
  `sudo umount`/`udisksctl`/`sudo zfs list`, 10 min for `wipefs`/`mkfs.*`, 20 s
  for the neighbour sweep. `run_timed_to_file` was added so `gunzip` keeps
  writing into its output file instead of a pipe.

Counts: `src/handlers/explorer.rs` **34 → 38** unit —
`test_path_helpers_see_files_dirs_and_a_missing_path`,
`test_module_deadlines_are_bounded_and_ordered`, and the two guards that make
the item verifiable at all:
`test_no_blocking_work_is_reachable_from_an_async_body` (walks the file and
fails on a blocking token in an `async fn` outside a `spawn_blocking` closure —
this is the defect class 64 integration tests could not see: a blocking call
still answers `200` on an idle box) and
`test_every_subprocess_of_this_module_carries_a_deadline` (no `.output()`,
`.spawn()` or `.status()` in production code). `src/browser.rs` **16 → 17**
(the subprocess half of the guard). `src/utils/process.rs` **10 → 12** —
`run_timed_keeps_a_caller_supplied_stdout` (the `gunzip > file` case), and
`archive_deadline_is_a_ceiling_not_a_probe_budget`. `tests/explorer_integration.rs`
**64 → 65** — `test_disk_scan_does_not_block_the_runtime`: a shim `lsblk`
sleeping 2 s is put first on `PATH`, `GET /explorer/disks` is started in a task,
and `GET /explorer/list` must be served **before** the scan returns; on the
`#[tokio::test]` single-threaded runtime the old shape could not serve anything
until the scan came back. The test then asserts the scan really reached the shim
(elapsed ≥ 1.5 s), so it cannot pass on a no-op scan.

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib` and `cargo check --test explorer_integration`.
`npm run check`/`npm run build` and the Playwright suites were not run (no
frontend file changed). `cargo clippy` on the touched modules is clean; the
tree's pre-existing clippy/fmt drift elsewhere (`ups.rs`, `disk.rs`, …) is
untouched.

## 2026-10-24 — Storage tools: a deadline on every `zpool`/`btrfs`/`mdadm`/`smartctl` call

ROADMAP X.2 for `disk.rs`, `pools.rs` and `snapshots.rs`. No behaviour a user
asks for changed; what changed is what happens when a pool, a disk or `sudo`
stops answering.

- `disk.rs` — its private deadline helper now delegates to
  `utils::process::run_timed_async_result`, and the 42 calls that were still
  bare (the scans and probes behind `/storage/disks`, `/storage/virtual-disks`,
  the RAID wizard's device facts, the SMART cards) go through one `bounded()`
  helper. `ProcessFailure::Spawn` maps to `DiskError::Io` and `Timeout` to
  `DiskError::Timeout`, so "not installed" is never shown as "too slow".
- `pools.rs` / `snapshots.rs` — were fully unbounded (they are synchronous):
  `POOL_QUERY_TIMEOUT` 30 s / `POOL_WRITE_TIMEOUT` 900 s and
  `SNAPSHOT_QUERY_TIMEOUT` 30 s / `SNAPSHOT_WRITE_TIMEOUT` 600 s.
- `utils::process` — `run_timed_with_stdin` and `run_timed_async_with_stdin`
  added: `sudo tee /etc/fstab` must keep its stdin, which `run_timed` closes
  (a closed stdin makes `tee` truncate the file to nothing). The input is
  written from its own thread, so a child that ignores it cannot deadlock the
  caller before the deadline loop starts.

Counts: `src/disk.rs` **66 → 70**, `src/pools.rs` **33 → 35**,
`src/snapshots.rs` **4 → 6**, `src/utils/process.rs` **12 → 16**,
`tests/storage_integration.rs` **28 → 29**.

The decisive new assertions are the three **source guards** —
`test_no_subprocess_of_this_module_runs_without_a_deadline` in each of
`disk.rs`, `pools.rs` and `snapshots.rs` — because this defect class is
invisible to a behavioural test: an unbounded call still answers correctly on
an idle box and only pins a blocking thread when the disk is wedged. They fail
on a bare `.output()`, `.spawn()`, `.status()` or `wait_with_output()` in
production code (`concat!` keeps the pattern out of the file's own source, so
the assertion cannot count itself). Around them:
`test_bounded_keeps_a_missing_tool_distinct_from_a_timeout`,
`test_bounded_reports_the_program_it_waited_for`,
`run_timed_with_stdin_feeds_the_child_and_keeps_stdout_piped`,
`run_timed_with_stdin_does_not_deadlock_when_the_child_ignores_stdin`,
`run_timed_to_file_does_not_pipe_stdout_back`,
`run_timed_async_with_stdin_feeds_the_child_and_bounds_it`, and the integration
case `test_a_slow_host_tool_is_waited_out_not_dropped` (a shim `sudo` sleeping
300 ms must be *waited for* — the scan comes back as an array after really
spending that time, so the test pins the bound without a 30 s run).

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib` and `cargo check --test storage_integration`.
`npm run check`/`npm run build` and the Playwright suites were not run (no
frontend file changed).

## 2026-10-24 — Cache mover: a deadline on `rsync`/`find`/`df`

ROADMAP X.2 for `cache.rs` (`encryption.rs` was checked in the same pass and
needed nothing — its entry was stale, every `cryptsetup`/`mount`/`umount`/`tee`
call already runs under `run_timed` with a per-subcommand deadline, ROADMAP 9).

Why this one matters more than its three call sites suggest: `run_mover` holds
the **process-wide** `MOVER_RUNNING` flag for the whole run, so an `rsync` that
never returned kept that slot set and no mover ran again — button or
`scheduler` — until a restart. The deadline is what lets the RAII guard release
it.

- `CACHE_QUERY_TIMEOUT` 30 s (`df -B1`, also the free-space gate that must
  answer before `rsync --remove-source-files` deletes anything),
  `CACHE_FIND_TIMEOUT` 600 s (`find` sizing / age list), `CACHE_RSYNC_TIMEOUT`
  6 h (a first drain of a full tier — a ceiling).
- One local `run_command(cmd, timeout)` helper over
  `utils::process::run_timed`; the timeout is a parameter so the kill path is
  testable (same shape as `disk::bounded`), and a non-zero exit stays *data*
  because the `rsync` path branches on exit 24.

Counts: `src/cache.rs` **33 → 38**, `tests/storage_integration.rs` **29 → 30**.

New assertions: `test_no_mover_subprocess_runs_without_a_deadline` (source
guard — a bare `.output()`/`.spawn()`/`.status()`/`wait_with_output()` in
production code fails it; the defect is invisible behaviourally, since an
unbounded call still answers correctly on an idle box),
`test_run_command_kills_a_hung_tool_and_names_the_deadline`,
`test_run_command_keeps_a_non_zero_exit_as_data`,
`test_run_command_reports_a_missing_tool`,
`test_mover_deadlines_are_ceilings_and_ordered`,
`test_df_space_answers_none_when_the_tool_fails`, and the integration case
`test_a_finished_mover_run_releases_its_single_flight_slot` (shimmed
`rsync`/`find`/`df`, `mover_is_running()` must fall back to `false`, and a
second `POST /cache/mover/run` must be accepted).

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib`, `cargo check --test storage_integration`.
`npm run check`/`npm run build` and the Playwright suites were not run (no
frontend file changed).

## 2026-10-24 — Network: a deadline on every `ip`/`wpa_cli`/`dhclient`/`netplan` call

ROADMAP X.2 for `network.rs` — the largest of the four modules left, and the
one the Network screen polls. The module runs on the blocking pool (its
handlers wrap it in `spawn_blocking`), so a wedged tool did not stall just one
request: `GET /network/interfaces`, `/wifi/status` and `/summary` are read on
every view of the screen, so the calls piled up behind the wedged one. ~38 call
sites, all now under a deadline.

- `NET_PROBE_TIMEOUT` 10 s (`ip … show`, `wpa_cli status`/`list_networks`/
  `scan_results`, `ethtool`, `sudo cat`, `ps`), `NET_WRITE_TIMEOUT` 30 s
  (`ip addr add|flush|route`, `dhclient`, `wpa_cli` connect/select/save,
  `netplan apply`, `sudo tee`/`chmod`/`rm` — root ops can wait on the sudo/PAM
  stack or systemd-networkd), `NET_PING_TIMEOUT` 5 s (`ping` carries its own
  `-W 2`).
- One local `run_command(cmd, timeout)` helper over
  `utils::process::run_timed`, plus `ip_sudo_run(program, args)` so a mutation
  cannot forget the `sudo` prefix **or** the deadline.
- `write_as_root` moved to `run_timed_with_stdin`: its hand-rolled `.spawn()` +
  `wait_with_output()` would hang forever on a `tee` that never read, and stdin
  is still written from its own thread. `run_command` keeps a non-zero exit as
  *data* (a `wpa_cli` refusal is reported on stderr).

Counts: `src/network.rs` **57 → 63**, `tests/network_integration.rs` **23 → 24**.

New assertions: `test_no_subprocess_of_this_module_runs_without_a_deadline`
(source guard — a bare `.output()`/`.spawn()`/`.status()`/`wait_with_output()`
in production code fails it; the defect is invisible behaviourally, since an
unbounded call still answers correctly on an idle box),
`test_run_command_kills_a_hung_tool_and_names_the_deadline`,
`test_run_command_keeps_a_non_zero_exit_as_data`,
`test_run_command_reports_a_missing_tool`,
`test_network_deadlines_are_ceilings_and_ordered`,
`test_wpa_cli_and_ip_sudo_prefix_sudo_and_keep_the_argv`, and the integration
case `test_a_slow_host_tool_is_waited_out_not_dropped` (a shim `ip` sleeping
250 ms must be *waited for* — `GET /api/network/summary` comes back `200` after
really spending that time, so the test pins the bound without a 10 s run).

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib`, `cargo check --test network_integration`.
`npm run check`/`npm run build` and the Playwright suites were not run (no
frontend file changed).

## 2026-10-24 — VPN: a deadline on every `wg`/`wg-quick`/`openvpn`/`pgrep` call

ROADMAP X.2 for `vpn.rs`. Every command was a bare `Command::output()` — except
the two stdin cases (`wg pubkey`, `wg set … preshared-key /dev/stdin`), which
hand-rolled `.spawn()` + `wait_with_output()`. A `wg` blocked on a missing
kernel module, an `openvpn --daemon` parent that never exits or a `pgrep` on a
huge process table pinned the calling thread, and the VPN screen polls the
server status and the peer list.

- `VPN_PROBE_TIMEOUT` 10 s (`wg show`/`--version`/`genkey`/`genpsk`, `pgrep`,
  `ip route`, `sudo cat`/`test`), `VPN_WRITE_TIMEOUT` 30 s (`wg set`,
  `wg-quick up`, `sudo chmod`, `systemctl enable`, `sudo kill`).
- One local `run_command(cmd, timeout)` over `utils::process::run_timed`; the
  stdin cases use `run_timed_with_stdin`.
- **New primitive:** `utils::process::run_timed_detached` for the
  `openvpn --daemon` parent. It cannot use `run_timed`: the daemon keeps the
  inherited write ends of any pipe, so the drain threads never see EOF and the
  deadline can never fire. The helper detaches both streams to `/dev/null`, waits
  only for the exit status, and kills at the deadline.

Counts: `src/vpn.rs` **29 → 34**, `src/utils/process.rs` **16 → 19**.

New assertions: `test_no_subprocess_of_this_module_runs_without_a_deadline`
(source guard, and it requires `run_timed_detached(` to stay in the daemon path),
`test_run_command_kills_a_hung_tool_and_names_the_deadline`,
`test_run_command_keeps_a_non_zero_exit_as_data`,
`test_run_command_reports_a_missing_tool`,
`test_vpn_deadlines_are_ceilings_and_ordered`, and the three `process.rs` cases
`run_timed_detached_returns_a_fast_command_status`,
`run_timed_detached_kills_a_command_that_overruns_its_deadline`,
`run_timed_detached_reports_a_missing_program`.

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib`. `npm run check`/`npm run build` and the Playwright
suites were not run (no frontend file changed).

## 2026-10-24 — Shares + Health: X.2 closed

ROADMAP X.2 for `shares.rs` and `health.rs`. For shares the handler already
wrapped the operation in `run_share_op` (`spawn_blocking` +
`tokio::time::timeout`), but that outer timeout only stops **waiting** for the
future — it does not kill the `sudo systemctl`/`exportfs`/`tee` child, which
keeps the blocking thread pinned. For health every check ran a bare probe on
the blocking pool, and the notification engine runs them every 60 s.

- `SHARES_WRITE_TIMEOUT` 30 s (`sudo tee`/`tee -a`, `sudo mkdir -p`; the two
  `tee` helpers moved to `run_timed_with_stdin`), `SHARES_SERVICE_TIMEOUT` 60 s
  (`systemctl` start/stop/restart/reload, `exportfs -ra`).
- `HOST_PROBE_TIMEOUT` 10 s (`df`, `lsblk`, `mdadm`, `systemctl is-active`) and
  `SMART_PROBE_TIMEOUT` 20 s (`smartctl`, through `smart::std_command`). A
  timed-out probe degrades that check to `Unknown`, never a fabricated value.

Counts: `src/shares.rs` **32 → 37** (recounted; the tracker's 34 was stale),
`src/health.rs` **17 → 22**, `tests/health_integration.rs` **15 → 16**.

New assertions: a source guard per module
(`test_no_subprocess_of_this_module_runs_without_a_deadline`), the kill path,
exit-status-as-data, a missing tool, and the deadline ordering in each; the
integration case `test_a_slow_host_probe_is_waited_out_not_dropped` (a shim
`df` sleeping 250 ms must be waited for — `GET /api/health` comes back `200`
after really spending that time). **No behavioural shares integration case**:
every host call of `shares.rs` is a mutation (writing `/etc` or starting a
service), which the suite must never do to the real host.

Written, **not run** (per instruction). What *was* run: `cargo check --lib`,
`cargo test --no-run --lib`, `cargo check --test health_integration`,
`cargo check --test shares_integration`. `npm run check`/`npm run build` and the
Playwright suites were not run (no frontend file changed).

## 2026-10-24 — UI action feedback (ROADMAP X.3)

The four areas X.3 named (CloudSync, Health, desktop power, NotificationCenter)
were already fixed; a scanner over `frontend/src` for empty/comment-only
`else`/`catch` bodies found the residual silent action failures it had missed:
Shares (~18 sites), Backup/restic (~15) and one Storage site (SMART self-test
start). All now report through the new shared
`frontend/src/lib/utils/notify.ts` (`notifyError` shows the server's
`message`/`error`; `notifySuccess('common.saved')` confirms config saves). Two
keys (`common.action_failed`, `common.saved`) were added to all four locales, so
`src/i18n.rs` key parity holds.

Counts: `tests/e2e/shares/shares.spec.ts` **51 → 53**,
`tests/e2e/backup/backup.spec.ts` **27 → 28**.

New assertions: shares 15.1 (a mocked refused share delete shows the reason in a
`[data-sonner-toast][data-type="error"]`), 15.2 (a mocked refused SMB config
save does the same), and backup 15.1 (a mocked refused repository delete shows
the reason; every restic route is mocked, nothing is deleted).

Written, **not run** (per instruction). What *was* run: `npm run check` (0
errors / 0 warnings) and `npm run build` (clean). No Rust or Playwright suite was
executed.

## 2026-10-24 — Test-count recount (ROADMAP X.5)

Every count in this file was re-derived from the tree, and the tracker rows were
reconciled. Method: `#\[test\]` / `#\[tokio::test\]` attributes for the Rust
suites, `grep -cE '^\s*test\('` for the Playwright specs, and
`npx playwright test --list --project=firefox` (which **lists**, never runs) for
the runtime figure.

**E2E totals: 1691 runtime tests across 43 spec files, 1616 by the `test(` grep.**
(2026-10-24, ROADMAP 31.1: `docker.spec.ts` 64 `test(` / 66 runtime → **67 / 69**, the new §17.
2026-10-25, ROADMAP 34.1: `design/typography.spec.ts` 7 `test(` / 11 runtime → **11 / 15**, the new §6.)
The difference comes from tests built in a `for` loop (one `test(` line, N
runtime cases). Every tracker row now carries the dual figure where the two
differ, in the Quotas style (`N runtime / M test(`):

| Spec | `test(` | runtime |
|---|---|---|
| `account/account.spec.ts` | 73 | 75 |
| `backup/backup.spec.ts` | 30 | 32 |
| `chat/chat.spec.ts` | 20 | 22 |
| `cloudsync/cloudsync.spec.ts` | 29 | 33 |
| `dashboard/dashboard.spec.ts` | 46 | 48 |
| `design/typography.spec.ts` | 11 | 15 |
| `docker/docker.spec.ts` | 67 | 69 |
| `downloader/downloader.spec.ts` | 46 | 48 |
| `explorer/explorer.spec.ts` | 62 | 64 |
| `groups/groups.spec.ts` | 61 | 63 |
| `health/health.spec.ts` | 35 | 37 |
| `journal/journal.spec.ts` | 30 | 32 |
| `login/login.spec.ts` | 97 | 101 |
| `network/network.spec.ts` | 48 | 50 |
| `password-change/password-change.spec.ts` | 14 | 16 |
| `quotas/quotas.spec.ts` | 41 | 43 |
| `raid/raid.spec.ts` | 43 | 45 |
| `rescue/rescue.spec.ts` | 57 | 59 |
| `security/security.spec.ts` | 41 | 43 |
| `settings/settings.spec.ts` | 111 | 114 |
| `setup/setup.spec.ts` | 96 | 100 |
| `shares/shares.spec.ts` | 53 | 55 |
| `storage/storage.spec.ts` | 40 | 48 |
| `syncthing/syncthing.spec.ts` | 30 | 34 |
| `terminal/terminal.spec.ts` | 77 | 81 |
| `users/users.spec.ts` | 94 | 96 |
| `vm/vm.spec.ts` | 45 | 47 |
| `vpn/vpn.spec.ts` | 39 | 41 |
| `widgets/widgets.spec.ts` | 23 | 25 |

**Rust rows:** 27 stale tracker figures were corrected to the attribute count
(the recent X.2/X.3 passes had updated the Done entries but not the rows):
`login_integration.rs` 62→61, `login_pam_integration.rs` 3→9, `setup.rs`
85→88, `setup_integration.rs` 60→59, `quota.rs` 21→20 (both rows),
`handlers/user_homes.rs` 7→8, `pools.rs` 24→35, `snapshots.rs` 4→6,
`storage_integration.rs` 28→30, `disk.rs` 66→70, `browser.rs` 11→17,
`handlers/explorer.rs` 17→38, `media_ticket.rs` 4→5,
`share_links_integration.rs` 11→12, `rescue/validator.rs` 34→25,
`rescue/jobs.rs` 5→4, `progress_integration.rs` 8→9, `handlers/chat.rs` 14→17,
`settings_persistence.rs` 7→15, `security_integration.rs` 25→26,
`settings_core_integration.rs` 28→32, `incus.rs` 14→22, `handlers/vm.rs` 17→21,
`vm_integration.rs` 24→26, `power_integration.rs` 4→9.

No test was executed. `--list` only enumerates what would run.

## Verification method

- `tests/*.rs` files listed via `ls`, counted via
  `grep -cE '#\[tokio::test\]|#\[test\]'` (number of test attributes =
  number of test functions).
- `tests/e2e/**/*.spec.ts` files listed via `find`, counted via
  `grep -cE '^\s*test\('` (the tests are indented in their `test.describe(` ;
  the pattern must be anchored at the start of the line). **Do not** use
  `grep -c 'test('`: it also catches the `.test(x)` of the regex
  assertions (`/…/.test(url)`) and over-counts. The number retained is
  that of Playwright — `npx playwright test <spec> --list --project=firefox`
  — which is the reference. **The two do not coincide, and that is normal**: a
  `test(` written **in a loop** is counted only once by the grep whereas
  Playwright expands it into N tests at collection (e.g. the responsive
  loop 360/768/1920 of `storage.spec.ts`: 1 line, 3 tests). The grep
  is therefore a **lower bound** ; the difference measures the tests generated by
  loop. Observed on 2026-10-19: **1467** at the grep against **1540** at
  `--list --project=firefox` (73 generated tests) on the 36 `.spec.ts` files.
- Unit tests per module: `grep -cE '^\s+fn test_'` in the source
  file, modules found via `grep -rl '#\[cfg(test)\]' src/`.
- The per-area statuses come from `CLAUDE.md` and from `docs/guides/` (not
  re-verified against the code in this pass — see the caveat of
  `ROADMAP.md` on the freshness of this type of information).

**2026-10-13 (b) — Explorer, Backup, VM and the 8-step Setup Wizard.** Four
feature sets landed together, each with its Rust tests; the Playwright specs
were **not re-run** (no fresh server in this session).

- **Explorer** — Rewind (per-file ZFS snapshot timeline, `GET /explorer/history`),
  server-side recents (`/explorer/recents` GET/POST/DELETE, migration
  `039_explorer_recents.sql`), fuzzy search (`?fuzzy=true`, `browser::fuzzy_score`),
  marquee-select, virtualized listing, internal drag & drop, SMB/network
  discovery (`/explorer/network-discovery`, `smbtree` + Avahi parsers), and
  ext4/exfat formatting (`/explorer/format`, two-value allow-list). `browser.rs`
  **11 → 15**, `handlers/explorer.rs` **17 → 22** (parsers + format allow-list),
  `explorer_integration.rs` **49 → 55**.
- **Backup** — reminders banner (`GET /restic/reminders`), split-view restore
  dialog with a debounced pre-flight space check (`GET /restic/restore-plan`),
  floating progress island (`backupJobs.svelte.ts` +
  `BackupProgressIsland.svelte`). `handlers/restic.rs` **+3** (`days_since`,
  `parse_df_available`, size/space comparison), `restic.rs` gains `restore_size`,
  `restic_integration.rs` **14 → 17**.
- **Virtual Machines** — Windows 11/10 + minimal Ubuntu/Debian catalogue entries
  with an `iso_only` flag, optional audio device, and "open the screen in a new
  tab". `incus.rs` **11 → 14**, `handlers/vm.rs` **14 → 17**,
  `vm_integration.rs` **22 → 24**.
- **Setup Wizard** — grew from 4 to 8 steps (welcome, language, storage, RAID,
  restore, account, password, verify) and became reboot-safe: the step + the
  non-secret choices persist in a singleton `setup_progress` row (migration
  `040_setup_progress.sql`) behind `GET/PUT /auth/setup/progress`, and
  `GET /auth/setup/storage-options` feeds the storage/RAID steps. Secrets are
  never part of the payload. `setup_integration.rs` **47 → 52**.

**2026-10-17 (g) — verification pass over all 8 points.** Every point was
re-checked on three axes (code wired, tests, live behaviour) against a fresh
instance. Seven passed as documented; **one real defect surfaced**:

> **A damaged `app_meta` refused the whole startup.** The schema guard reads
> `app_meta.schema_version` *before* anything else, and `stored_schema_version`
> propagated the SQLite error — so corrupting a single page in that table made
> `run_migrations` fail and the process exit, which is the exact opposite of the
> documented "degrade rather than refuse to boot" rule (and happens precisely
> when the operator needs the appliance up to restore a snapshot).
> `stored_schema_version` now treats an unreadable catalogue as *unknown*
> (warn + `Ok(None)`), so the guard passes, `quick_check` reports the damage and
> `/api/health/ready` answers **503 degraded** while the appliance stays
> reachable. Regression test: `test_unreadable_app_meta_is_treated_as_unknown_not_fatal`.

Live evidence collected during the pass (fresh instance, `hv-final`):

| Point | Check | Result |
|---|---|---|
| 1 | `POST /settings/backup/db/now` → snapshot opened standalone | 59 tables, `users=['verif']`, `quick_check=ok`, no `-wal` needed |
| 2 | forced `schema_version=999` → start | `ERROR Refusing to start…` + **exit 1**, port closed |
| 3 | `/health/live`, `/health/ready` without a session | `200 {"status":"ok"}` / `200 {"status":"ready"}` |
| 4 | CSP header vs stamped HTML, two requests | nonce matches, differs per response, no `unsafe-inline`, 0 console violations, SW registers |
| 5 | `Set-Cookie` over HTTP; banner LAN vs loopback | `HttpOnly; SameSite=Lax`, **no** `Secure`; banner `role="status"` on `homevault.test`, absent on `localhost` |
| 6 | `src/middleware/fail2ban.rs` | gone; no `Fail2BanState` reference anywhere |
| 7 | corrupted page → `/ready` | `503 {"status":"degraded","checks":{"integrity":"… malformed"}}` |
| 8 | installer in a sandbox copy | rollback swaps binaries, `--uninstall` keeps data, `--purge` deletes after `PURGE`, `--dry-run` works unprivileged |

`cargo test` **54 suites / 0 failures**, lib **847** (+1 regression test);
`npm run check` 0 errors, `npm run build` OK, `bash -n installer/install.sh` OK.


**2026-10-17 (f) — production readiness, lots C/D (CSP nonce, installer lifecycle).**

- **CSP: `script-src` no longer allows `'unsafe-inline'`.** New
  `middleware/csp_nonce.rs` generates 16 random bytes per request,
  `security_headers_middleware` puts them on the request extensions *and* in the
  policy, and `handlers::spa::serve_spa` replaces the `__HV_CSP_NONCE__`
  placeholder in `index.html` with the same value. The two must agree within one
  response. `style-src` keeps `'unsafe-inline'` (Svelte scoped styles +
  `style=""` attributes) — recorded as accepted debt.
- **Installer lifecycle:** `--rollback` (swaps in
  `/opt/homevault/homevault.previous`, kept by every upgrade), `--uninstall`
  (keeps `/var/lib/homevault`), `--uninstall --purge` (typed `PURGE`
  confirmation), `--dry-run` (faithful preview via a single `run()` helper,
  works without root). `--rollback` rolls back the binary only and prints the
  exact commands to restore the pre-migration snapshot, because migrations are
  append-only and the older binary refuses a newer schema.

Tests: `csp_nonce.rs` **+3** (hex/uniqueness, multi-occurrence stamping,
no-op without the placeholder), `spa_assets_integration.rs` **+5**
(`unsafe-inline` absent from `script-src`, header nonce == stamped HTML,
fresh nonce per response, stamping, 128-bit shape). Verified live in a browser:
shell mounts, **service worker registers** (proving the nonce'd inline script
runs) and **0 CSP violations** in the console. Installer verified in a sandbox
copy with rewritten paths: `--uninstall` keeps the data, `--purge` with a wrong
confirmation aborts, `--purge` with `PURGE` deletes, `--rollback --dry-run`
prints the exact moves, `--dry-run` works unprivileged. `bash -n
installer/install.sh` OK. `cargo test` all green (lib **846**); Playwright specs
not re-run.


**2026-10-17 (e) — production readiness, lots B/C (dead code, TLS).**

- **Removed `src/middleware/fail2ban.rs`.** It defined an in-memory IP ban list
  that no route or handler ever referenced (its own header said so). Dead code
  that reads like an active protection is worse than no code: it stops anyone
  from looking for the real defence. `docs/guides/security.md` now records that
  brute-force protection is the per-account lockout plus the per-IP throttle, and
  that an unwired "protection" module must not come back.
- **HTTPS now actually protects the login.** `TlsSettings::default().redirect`
  is `true` (it was `false`), with `#[serde(default = "redirect_default")]` so a
  `data/tls.json` written before the field existed deserializes to the safe value
  instead of silently reading `false`. `app_proxy::cookie::attach_session` gained
  an `over_tls` flag and emits `Secure` only when the request really arrived over
  TLS (unconditional would break the app proxy on a plain-HTTP LAN). The wizard
  and the login screen show a `role="status"` warning when served over `http://`
  on a non-loopback host; loopback is exempt. The installer's HTTPS question now
  defaults to **yes**.

Tests: `tls.rs` **+2** (redirect-on default, legacy JSON picks up the default),
`app_proxy/cookie.rs` **+3** (no `Secure` on plain HTTP, `Secure` over TLS,
`Secure` survives `remember_me`), `login.spec.ts` **+2** (§26 banner shown for a
LAN host via `--host-resolver-rules`, absent on loopback). Verified live: the
`Set-Cookie` on HTTP has no `Secure`; the banner renders with `role="status"`
for `http://homevault.test:8090` and is absent for `http://localhost:8090`.
`cargo test` all green (lib **843**); `bash -n installer/install.sh` OK;
`npm run check` 0 errors, `npm run build` OK. Playwright specs edited but **not**
re-run.


**2026-10-17 (d) — production readiness, lot A (database safety).** Three fixes,
each with tests. The first was found by measuring, not by reading:

1. **The database backup was silently broken.** The database runs in WAL mode and
   nothing ever checkpointed it, so `homevault.db` was **4 KB** while
   `homevault.db-wal` held **1.8 MB** — copying "the database file" produced a
   file with **no tables at all**. Snapshots are now written with
   `VACUUM INTO` (self-contained single file): at startup when due, every 15 min
   when due, and **before any pending migration**. Settings → System gained a
   *Database snapshots* card (last run, schedule, retention 1–100, list of files,
   "Snapshot now"). The scheduler also runs `PRAGMA wal_checkpoint(TRUNCATE)` so
   `-wal` stays bounded (measured: 33 KB after a run).
2. **An older binary silently ran on a newer schema.** `app_meta.schema_version`
   (migration `048`) is compared with the highest embedded migration; a newer
   file is a fatal startup error with an actionable message. `src/bin/server.rs`
   propagated nothing before — it swallowed the error and exited **0**, so
   `systemd Restart=on-failure` could not see a refused start. Now it exits 1.
3. **A damaged database was only discovered at the first query.** `PRAGMA
   quick_check(1)` runs at boot and is surfaced through the new public
   `GET /api/health/ready` (503 `degraded`), which also reports an unreachable
   database. `GET /api/health/live` answers 200 without touching the DB. Both are
   deliberately terse — no hostname, disk, version or machine id.

Measured live on a fresh instance: startup snapshot written
(`homevault-…db`, 700 416 bytes), the copy opened standalone with **59 tables**
and `schema_version = 48`, `/ready` → `200 {"database":"ok","integrity":"ok"}`,
and a forced `schema_version = 999` → `ERROR Refusing to start: … newer
HomeVault …` + **exit 1**.

Tests: `storage/database.rs` **+10** (schema-version round-trip, newer/older/
missing `app_meta`, `VACUUM INTO` self-containment, existing destination,
`quick_check` healthy + corrupted + `wal_checkpoint` truncation),
`db_backup.rs` **+5** (name shape, settings bounds, ordering),
`db_backup_integration.rs` **new, 12** (auth/admin on all three endpoints,
snapshot written + listed, retention pruning, probes with and without a
session, degraded readiness, liveness leak check), `settings.spec.ts`
**+3** (§15 card, snapshot round-trip, keyboard/accessible name).
`cargo test` all green (lib **838**); `npm run check` 0 errors, `npm run build`
OK. Playwright specs edited but **not** re-run.


**2026-10-17 (c) — login hardening: the lockout no longer bricks the account.**
Audit of the Login screen found two production blockers, both reproduced live on
a fresh instance:

1. **Permanent lockout.** Five wrong passwords set `users.locked = 1` and nothing
   ever cleared it — after the 5-minute window the *correct* password still
   answered "Account is locked". On a single-admin appliance the only ways back
   were the offline recovery code or editing the database. Fixed with
   `users.locked_until` (migration `047`): an automatic lock expires on its own
   and a successful login clears it; a deliberate admin lock keeps
   `locked_until = NULL` and stays permanent. `set_user_locked` and the
   recovery-code reset clear the column explicitly.
2. **No per-IP throttle.** The only counter was per account, so spraying one
   password across 20 usernames produced 20 × 401 with no limit.
   `AuthService::check_ip_rate_limit` now runs before the account lookup on
   `/auth/login` and `/auth/2fa/verify` (30/min, 120/hour per IP → 429).

Also: the client IP comes from the TCP peer (`middleware::PeerAddr` +
`handlers::auth::client_ip`), with `X-Forwarded-For` honoured only from loopback
or `HOMEVAULT_TRUSTED_PROXIES` — it was client-forgeable in the login history.
The handler now returns the status it computes (429 instead of a blanket 400).
On the screen: the inline error is a `role="alert"`/`aria-live="assertive"`
region, the fields carry `aria-invalid` + `aria-describedby`, closing the error
modal restores focus to the form (it landed on `<body>`), and the locked message
points at **Forgot password?** + recovery code instead of "contact support".

`handlers/auth.rs` **+6** (`client_ip`), `login_integration.rs` **54 → 60**,
`login.spec.ts` **92 → 95**. Two E2E passwords that had silently become invalid
under the 12-char policy (`OldRecP1a`/`NewRecP2b`, which made the whole §24
recovery suite skip itself) were lengthened. Re-run locally: `cargo test`
**all green** (lib 822, login 60, users 61, account 25, auth_recovery 7,
setup 60); `npm run check` 0 errors, `npm run build` OK. Playwright **not**
re-run; the flow was verified live in a real browser against a fresh instance
(lock recorded with an expiry, expiry lifts the lock, spraying throttled at 30,
`aria-invalid`/`role=alert` present, focus back on `#username`).

**2026-10-17 (b) — the Setup Wizard dropped its storage, RAID, restore and
language steps.** The wizard is down to **4 steps** (welcome, account,
password, verify): RAID is configured from its own app, the appliance is
installed by script on an existing OS, a backup is restored from the Backup app,
and the interface language is now detected instead of asked. Removed:
`GET /auth/setup/storage-options` (`handle_setup_storage_options`), the frontend
`getSetupStorageOptions` service method, the storage/RAID/restore/language step
markup + detection state in `SetupWizard.svelte`, the `setup.storage.*` /
`setup.raid.*` / `setup.restore.*` / `setup.language.*` /
`setup.stepper.{storage,raid,restore,language}` i18n keys (all four locales),
and the `test_setup_storage_options_first_run_only` integration test.
`setup_progress.data` now carries nothing; `MAX_SETUP_STEP` is 3.

**Language detection replaces that step.** `SetupService::detect_system_language()`
(`src/setup.rs`) maps the host POSIX locale (`LC_ALL` → `LC_MESSAGES` → `LANG` →
`LANGUAGE`, then `/etc/default/locale`) to `en`/`fr`/`de`/`es` — English for
anything unsupported — and `GET /auth/setup-status` exposes it as
`system_language`; `languageStore.applySystemLanguage()` applies it only when
the user has no saved choice and the browser locale is not one we ship.
`setup.rs` **45 → 50** unit tests (territory/case/priority-list/unsupported
`language_from_locale` cases + a `detect_system_language` membership check),
`setup_integration.rs` **51 → 51** (the storage-options case left, a
`system_language` case arrived). The Playwright spec was renumbered to the
4-step flow (account 1, password 2, verify 3) via a `gotoStep` helper, with new
§18b/§18c running under `fr-FR` and `it-IT` Playwright locales to pin the
detection and its English fallback, but **not re-run** (no fresh first-run
server in this session).

**2026-10-17 (c) — production hardening of the first-run flow.** Five fixes,
each with tests:

1. **Same-origin guard** on the bootstrap routes (`middleware/setup_guard.rs`).
   The permissive CORS layer meant a cross-site page in the operator's own
   browser could claim the first admin; `Origin` must now match `Host`,
   `Origin: null` is refused, and `Sec-Fetch-Site` must be `same-origin`/`none`
   (header-less non-browser clients still pass).
2. **Per-peer rate limit** (20 req/min) on the guarded routes, in a `OnceLock`
   inside the guard so `AppState` fixtures need no change.
3. **Self-healing setup lock**: `pid` + `created_at` in `.setup_lock`, a dead PID
   or an age over 15 min is reclaimable, and `cleanup_stale_setup_lock()` runs
   at startup — a crashed setup no longer dead-ends the wizard.
4. **Password policy 8 → 12 chars** plus a compromised/obvious-password list and
   a repeated-run rule, in `SetupService::validate_password`, with
   `AuthService::validate_password_strength` delegating to it. The wizard's
   checklist grew from 4 to 6 rules and is what gates its "Next" button.
5. **Blocking recovery-code screen**: a failed generation offers a retry, never
   a "continue".

`setup.rs` **50 → 61** unit, `setup_integration.rs` **51 → 60**,
`setup.spec.ts` **93 → 96** (`grep -cE '^\s*test\('`; 5.7/5.8 count 6 checklist
rows, 5.13-5.15 pin the new password rules). Password fixtures in
`account_integration.rs`, `auth_recovery_integration.rs` and
`login_integration.rs` were lengthened to 12+ (the policy is shared). Specs and
suites re-run locally: `cargo test --lib` **816/816**,
`setup_integration` **60/60**, `account_integration` **25/25**,
`auth_recovery_integration` **7/7**, `login_integration` **54/54**,
`users_integration` **61/61**. Playwright **not** re-run (no fresh first-run
server in this session); the flow was instead verified live in a real browser
against a fresh instance.

`cargo test` on 2026-10-13 (b): `--lib` **734**, `explorer_integration` **55**,
`restic_integration` **17**, `setup_integration` **52**, `vm_integration` **24**,
`rag_indexers` **3/3** (FeatureDoc coverage still green after the
explorer/setup/vm/backup entries were refreshed). `npm run check` 0 errors
(1 pre-existing a11y warning), `npm run build` OK.

To be updated on each new test file added or new manual audit
documented in `CLAUDE.md`.

**2026-10-14 — the WebDAV server became part of HomeVault (Rust).** The
generated Python script + root-owned `homevault-webdav.service` were replaced
by the in-process `src/webdav/` module (`dav-server` over hyper 1.x), which
also fixes three real defects: Basic auth now verifies the HomeVault
credentials (the Python `_check_auth` accepted any `Authorization: Basic`
header), the URL's first segment is a **share name** confined to that share's
root (the old server ignored `path_prefix` and served the process CWD as
root), and `PROPFIND` is authenticated. Per-member `share` grants from
`resource_shares` are now enforced on every operation.

- **New unit tests — `src/webdav/` (27):** `fs.rs` 11 (share-name/path
  normalization, `..` and NUL rejection, servable-share filtering and
  de-duplication, `is_inside`), `vfs.rs` 11 (`GuardedFileSystem` grant checks,
  cross-share `COPY`/`MOVE` refusal, non-ASCII path re-encoding, real
  read-dir/metadata/open against a `tempfile` share), `serve.rs` 4 (Basic
  header parsing incl. malformed base64 / empty user / wrong scheme, prefix
  normalization), `mod.rs` 1.
- **`tests/shares_integration.rs` 18 → 20:** `POST /shares/webdav/start` is
  admin-only and, with no WebDAV share defined, reports success without
  binding a socket (port 0); `GET /shares/webdav/config` defaults to a
  bindable port with `require_auth: true` (the old derived default was port 0
  + auth off).
- **Not covered by Rust tests** (needs a live host / real client): an actual
  DAV session over the wire, TLS termination via `require_ssl`, and the
  `dav-server` conformance suite (`litmus`). `tests/e2e/shares/shares.spec.ts`
  is unchanged and was **not re-run** (the WebDAV markup did not change).
- `cargo test` on 2026-10-14: full suite green; `shares_integration` **20/20**;
  `cargo clippy --lib` shows only the pre-existing `pools.rs` /
  `handlers/auth.rs` warnings.

**2026-10-24 — DB backup: the serialisation is process-wide, a failure is
visible, and retention spares the restore point (ROADMAP 22).** Three
behavioural assertions were added, each one aimed at the defect it closes —
neither was reachable with the previous "auth + list shape" tests.

- **`src/db_backup.rs` 5 → 11 unit tests.** New:
  `test_pre_migration_name_is_recognised` (the marker, including a `_2`
  duplicate), `test_retention_never_prunes_the_pre_migration_restore_point`
  (a restore point older than the kept window survives, and the regular
  snapshot above it is the one deleted),
  `test_retention_does_not_count_the_restore_point_against_retain` (it used to
  consume one of the `retain` slots, so `retain = 2` kept only one real
  snapshot), `test_pre_migration_restore_points_are_bounded_separately` (six
  restore points → the newest three, with `retain = 100`),
  `test_unique_snapshot_name_keeps_the_sort_order` (a same-second duplicate is
  `_2`, and still sorts newest-first), and
  `test_a_snapshot_waits_for_the_process_wide_lock` — it holds the module
  static and proves `snapshot_into` blocks on it, from a **different service
  instance** than the one holding the lock (which is what every handler
  builds). That test fails against the old per-instance `Arc<Mutex<()>>`.
- **`tests/db_backup_integration.rs` 12 → 15.** New, all over the real Axum
  router + a file-backed SQLite (a `VACUUM INTO` from `:memory:` writes
  nothing, so a file is what exercises the path):
  `test_retention_never_prunes_the_pre_migration_restore_point` (three manual
  snapshots at `retain = 2` with a restore point planted on disk → two regular
  files + the restore point, flagged `is_pre_migration`),
  `test_pre_migration_restore_points_are_bounded_separately` (six planted
  files + a settings save → the newest three survive),
  `test_a_failed_snapshot_is_reported_and_cleared_by_the_next_success` (the
  backup directory is replaced by a plain file → `POST /now` is `500`,
  `GET` reports `last_error` naming the directory, removing the obstruction
  makes the next attempt succeed **and** clears the field). `last_error` was a
  hardcoded `None` before, so the second half of that test could not pass.
- **`settings.spec.ts` §15 3 → 4 `test(`.** New: 15.4 — a status carrying
  `last_error` renders a `role="alert"` line with the server's own words
  (`page.route` on `GET /api/settings/backup/db`, so it does not need a broken
  volume).
- **Not covered:** no test performs a real concurrent `VACUUM INTO` against a
  live appliance; the lock invariant is pinned by the unit test holding the
  static, and by the router tests that go through it. Written on 2026-10-24,
  **not run** (per instruction).

**2026-10-24 — Server control: one privilege path, and a refusal the user can
see (ROADMAP 24).** Both findings were about *feedback* and *privilege*, so
the new tests assert exactly those two things — a status-code check would have
passed against the broken tree.

- **`src/handlers/system.rs` 0 → 4 unit tests.** New:
  `server_endpoints_use_the_shared_power_primitive` (the argv is
  `sudo systemctl reboot|poweroff`, and the module source contains no
  `process::Command` of its own — a second hardcoded `shutdown -r now` fails
  the test), `status_labels_are_stable` (the `restarting` /
  `shutting_down` contract the desktop reads), `accepted_command_answers_its_status`
  (`200`, `data.status`, `data.message`), `refused_command_carries_the_host_reason`
  (`500`, `success:false`, the host's stderr inside `message`, no `data`) and
  `a_host_task_failure_is_a_refusal` (a `spawn_blocking` join failure is never
  a fake success).
- **`src/power.rs` 5 → 6 unit tests.** New
  `every_power_action_goes_through_sudo_systemctl`: every `PowerAction` builds
  the same `sudo systemctl <verb>` command (`host_power_argv` /
  `host_power_command`), so a caller cannot reintroduce a second path.
- **`src/setup.rs` +1 unit test.**
  `test_power_grant_names_the_systemctl_verbs_only` asserts the sudoers line
  the template writes is exactly the three verbs the primitive runs — built
  from `PowerAction`, so adding an action cannot leave the grant behind — and
  that the bare `shutdown` binary is **not** granted.
- **`tests/power_integration.rs` 4 → 9.** New:
  `server_control_and_power_share_one_admin_gate` (anonymous and non-admin
  refused on `/api/server/restart|shutdown` and `/api/power/reboot|shutdown`),
  `an_invalid_session_is_rejected_on_every_power_route` (`401` on five routes,
  unknown session never reaches the host), `server_control_routes_only_answer_post`
  (`405` on `GET`, so the actions are not reachable as resources),
  `a_refused_host_command_reaches_the_response_body` and
  `an_accepted_host_command_answers_its_status` (the mapper both handlers use,
  asserted directly — an admin POST on these routes would take the test host
  down, so no test sends one).
- **`tests/e2e/desktop/power.spec.ts` new — 5 `test(` / 10 runtime.** The
  desktop power flow with every `POST /api/server/*` intercepted by
  `page.route` (never a real reboot): 1 a refusal renders a `role="alert"`
  with the server's words and the power menu is usable again (and the mock was
  really the thing that answered), 2 an accepted restart renders a `role="status"`
  band and the confirm button is disabled while the request is in flight (one
  click → one request), 3 the dismiss control is 44px, has an accessible name
  and a visible focus ring, 4 a dropped connection is **not** reported as a
  refusal, 5 a long refusal wraps at 400 px without widening the page.
- **Not covered:** no test executes a real `systemctl reboot|poweroff|suspend`;
  the sudoers grant is asserted at the template level, not by running `sudo -l`
  on a live host. `settings/power.spec.ts` keeps its read-only contract. Written
  on 2026-10-24, **not run** (per instruction).

**2026-10-24 — Autostart: off the async runtime, and a refusal that says why
(ROADMAP 25).** Both findings were about *where* the call runs and *what the
caller is told*, so the new tests assert the argv, the three probe states and
the reason string — a status-code-only test passes against the broken tree
(the old handler answered `400` with a generic message either way).

- **`src/autostart.rs` 0 → 9 unit tests.** New: `args_for_builds_the_scoped_sudo_argv`
  (exactly `systemctl enable|disable homevault`, and `is-enabled homevault`),
  `the_unit_name_is_a_constant_never_user_input` (the sudoers-scoped grant is
  only safe because nothing from a request reaches the argv),
  `enabled_from_output_reports_enabled_on_success`,
  `enabled_from_output_reads_a_disabled_unit_as_false` (exit 1 + clean stderr
  is systemd answering "disabled"), `enabled_from_output_surfaces_a_host_that_cannot_answer`
  (exit 1 + stderr is `Err`, **the** assertion that catches 25.2's
  `unwrap_or(false)`), `set_enabled_outcome_returns_the_stderr_tail`
  (`polkit refused: …` verbatim), `set_enabled_outcome_explains_a_silent_non_zero_exit`
  (an empty stderr still names the action and the exit status),
  `set_enabled_outcome_is_ok_on_success`, `deadlines_are_bounded_and_ordered`
  (`AUTOSTART_PROBE_TIMEOUT` ≤ `AUTOSTART_UPDATE_TIMEOUT`, both ≤ 60 s).
- **`tests/settings_core_integration.rs` 24 → 28.** `/settings/autostart` joins
  the no-token / bogus-token / non-admin matrices on both verbs (a global
  scriptable fake would have to match every captured request, so the route was
  added to the loops instead of mocking). Four new tests use a `PATH` shim (a
  `#!/bin/sh` stand-in for `systemctl` / `sudo` in a temp dir prepended to
  `PATH`, restored on drop — safe because every test in this binary serialises
  on `ENV_LOCK`): `autostart_read_reports_a_disabled_unit_as_false`,
  `autostart_read_distinguishes_a_host_that_cannot_answer` (`500` + the
  systemd reason, never a false `enabled:false`),
  `autostart_update_surfaces_the_hosts_refusal_reason` (`500` whose body
  contains `polkit refused: no authorization`), and
  `autostart_update_records_a_successful_toggle`. **No host command runs.**
- **`tests/e2e/settings/settings.spec.ts` 104 → 106 `test(` / 109 runtime.**
  3.5 mocks `PUT /api/settings/autostart` to a `500` and asserts the card
  renders a `role="alert"` with that exact reason (`data-testid="autostart-error"`),
  the rail Save parked on `data-state="error"`, and the switch **reverted** to
  the server-accepted value. 3.6 mocks GET/PUT to success and asserts the
  switch reflects the served `enabled` with no error band. 3.4's PUT assertion
  now accepts `200` or `500`: since 25.2 a host refusal is explicit, so the old
  "never 5xx" expectation was the bug's own contract.
- **Not covered:** neither test runs a real `sudo systemctl enable homevault`;
  the sudoers grant is asserted at the argv/constant level, not with `sudo -l`.
  Written on 2026-10-24, **not run** (per instruction).

**2026-10-24 — Wallpapers / Themes: one appliance look every account can read,
a validated wallpaper, and two no-op fields deleted (ROADMAP 26.1/26.2).** The
defect was an *authorisation* defect with a silent fallback, so the tests assert
who can read, what the payload contains, and what a refusal does to the stored
blob — not a status code.

- **`handlers/settings.rs` 25 → 29 unit.** `test_appearance_no_longer_carries_theme_or_density`
  (the model serialises exactly two fields, and an **old payload that still
  sends** `theme`/`density` deserializes instead of being refused);
  `test_validate_wallpaper_accepts_every_client_shape_and_normalises`
  (`/wallpapers/hv-07.webp`, `hv-07`, the `WallpaperConfig` JSON, a bare JSON
  string, whitespace, the legacy `{"type":"default"}` → `hv-01`, `""`);
  `test_validate_wallpaper_refuses_anything_off_the_bundled_list` (`hv-00`,
  `hv-45`, `hv-7`, `../`, a custom upload, another host, a data URL, a colour,
  a gradient, non-JSON — every message starts `wallpaper:`);
  `test_validate_accent_color_accepts_hex_and_refuses_injection`
  (`#fff;background:url(x)` and `var(--hv-danger)` are refused).
- **`settings_persistence.rs` 13 → 15 unit.**
  `test_legacy_theme_and_density_on_disk_load_and_are_ignored` (a
  `settings.json` from an older release imports and loads, and a re-export no
  longer writes the two keys);
  `test_apply_loaded_settings_drops_an_off_list_wallpaper` (an off-list stored
  value becomes `""`, the accent survives, and a valid padded value survives
  normalised).
- **`src/i18n.rs` 8 → 9 unit.** `settings.wallpaper_managed_by_admin` and
  `settings.appearance_load_failed` exist and are non-empty in **all four**
  locales; `settings.density` is gone from all four (key parity was 3254 each
  before, 3255 after).
- **`tests/settings_core_integration.rs` 24 → 28.**
  `test_appearance_is_readable_by_a_non_admin_session` — **the assertion that
  would have caught 26.1** (a member gets `200` + the admin-set accent and
  wallpaper, the payload is exactly two keys with no `smtp`/`password`/`webhook`/
  `check_frequency` string, and no-token/bogus-token stay `400`/`401`);
  `test_a_non_admin_cannot_change_the_appliance_look` (a member `PUT` is `400`
  and the shared value is unchanged); `test_a_malformed_wallpaper_is_refused_and_leaves_the_stored_value`
  (four off-list shapes + a bad accent are `400`s naming the field, and the
  stored wallpaper/accent are untouched — both fields are validated before the
  first write); `test_a_json_encoded_wallpaper_config_is_normalised_on_save`.
  `/settings/appearance` left the admin-read matrix (it is a session read now);
  it is asserted on its own. The PUT→GET test asserts the two-field shape.
- **`tests/e2e/settings/settings.spec.ts` 104 → 111 `test(` / 114 runtime**
  (`playwright test --list` per project = **114**; the +3 gap is the responsive
  loop of §4.7). New members are real accounts (`POST /api/users`, role 1),
  deleted in `afterAll`:
  - **4.8** — a **non-admin** desktop paints the server accent (`#e2725b` read
    from `getComputedStyle(:root).--hv-primary`) although this origin's
    `localStorage` says `hv-02`, and the server wallpaper replaces the stale
    local one. The regression test for 26.1.
  - **4.9** — a member reads `200` with exactly `['accent_color','wallpaper']`,
    a `PUT` is `400`, and the grid renders read-only
    (`[data-testid="appearance-read-only"]` visible, `data-readonly="true"`,
    every tile `disabled`).
  - **4.10** — no `#density`/`#theme` control, no `data-theme`/`data-density`
    on `<html>`, and the Save PUT body is exactly
    `['accent_color','wallpaper']`.
  - **4.11** — a member's Settings load does not depend on the admin aggregate.
  New shared helpers `createMember` / `deleteMember`.
- **Not covered:** no test proves the toast on a *failed* hydration (it needs
  the appearance endpoint to fail while the desktop still boots — the server
  refusing `GET /settings/appearance` would have to be mocked at the Playwright
  layer, and the fallback path is the same one the pre-26.1 code always took).
  Written on 2026-10-24, **not run** (per instruction).

## 2026-10-24 — One auth primitive, one status contract (ROADMAP X.7 + X.8)

**What was wrong.** Two independent defects, both invisible to a status-only
test around a *valid* token:

- **X.7 — hand-rolled auth checks.** `handlers::admin_guard` was meant to be the
  shared session check, but nine modules carried their own copy:
  `handlers::auth::require_admin` (a byte-for-byte duplicate) and private
  `require_admin` helpers in `syncthing`, `shares`, `vpn`, `ups`,
  `permissions`, `snmp`, `user_homes` and `downloader`, plus inline
  `x-session-token` blocks in `security.rs` (6), `settings.rs` (20),
  `explorer.rs` (31), `chat.rs` (7) and `avatars.rs` (1), plus 11 dead
  re-checks in `docker.rs` that ran *after* a successful guard. The copies had
  drifted: `shares`, `permissions`, `snmp` and `settings` answered **`400`** to
  a signed-in non-admin while every other module answered **`403`**.
- **X.8 — a missing token was `400`.** `require_auth` answered
  `json_error` (`400 Bad Request`) when the header was absent and `401` only for
  an invalid/expired one, so a client branching on the status read an
  authentication failure as a validation error.

**What changed.** `admin_guard` is now the only module that turns
`x-session-token` into an identity decision. It gained `require_auth_caller`
(username + own token, for the self-service session endpoints),
`require_session_token` (the modules that still resolve the `Session` row
themselves) and `denial_message` (status → wording, for modules that answer a
`Json<ApiResponse>` envelope). Every copy now delegates; the `docker.rs` dead
blocks are gone; the inline blocks in `settings`/`explorer`/`chat`/`avatars` go
through the guard. The status contract is `401` (header absent/empty, or
unknown/expired token), `403` (valid non-admin on an `admin` route), `400` only
for a request the server could not understand — stated once in
`docs/API.md` → *Auth status contract*.

**Tests added (written, not run — per instruction).**

| Suite | Count | What it pins |
|---|---|---|
| `src/handlers/admin_guard.rs` (unit) | **0 → 4** | source guards: no module defines its own `require_admin`/`require_auth`; the eight delegated modules contain zero `x-session-token` reads; `security.rs` never parses the header; `denial_message` matches its status |
| `tests/auth_guard_integration.rs` (new) | **7** | the contract end to end: missing/empty/unknown token → `401` on every guarded route; signed-in non-admin → `403` on an `admin` route and `200` on an `auth` route; malformed body from an admin → `400` **on the same route** that answers `401` without a session; an unparseable body is a clean `4xx`, never a panic |
| `tests/account_integration.rs` | **30 → 31** | `test_security_endpoints_reject_an_expired_session` — the assertion X.7 needed: a hand-rolled copy can lose the expiry predicate (exactly what item 15 had to fix on the Dashboard), and no test around a *valid* token would notice |
| `tests/shares_integration.rs` | 26 | the non-admin matrix now asserts **`403`** instead of ignoring the status, and `/users/{u}/permissions` on another account is `403` |
| `tests/snmp_integration.rs` | 8 | the non-admin matrix now asserts **`403`** (it pinned the drifted `400`) |
| `tests/settings_core_integration.rs` | 32 | the non-admin read/write matrices now assert **`403`**; the no-token matrix asserts **`401`** |

**Status sweeps in existing suites** (no new test, the assertion changed):
`activity`, `chat`, `dashboard`, `db_backup`, `encryption`, `journal`,
`network`, `notifications`, `power`, `progress`, `quotas`, `raid`, `rescue`,
`resource_shares`, `restic`, `security`, `settings_core`, `snmp`, `storage`,
`tls`, `vm`, `weather_location`, `whats_new`, `widgets` — **39 `400` assertions
flipped to `401`** where the request carried no token, and the two tolerant
`UNAUTHORIZED || BAD_REQUEST` pairs (`power`, `db_backup`) tightened to a strict
`401` now that the two families agree.

**Why the count is exact.** `grep -c '#\[tokio::test\]'` on the new suite gives
7 and on `account_integration.rs` 31; `grep -c '^    #\[test\]'` on
`admin_guard.rs` gives 4. No Playwright spec changed — the SPA never branched on
`400` vs `401` (checked: `frontend/src/lib/services/api.ts` only special-cases
`426`), so this pass adds no E2E case.

**Not verified.** Per instruction the suites were **not executed**; `cargo test
--no-run` compiles every target (57 integration binaries + lib), and
`cargo check --all-targets` is clean. The `401`/`403` assertions above were
derived by reading each call site and the guard it reaches, not by a green run —
a full `cargo test` is the acceptance step for closing X.7/X.8.

## 2026-10-24 — Activity taskbar and mDNS (ROADMAP 28.1, 28.2, 29.1, 29.2)

Four defects in two self-contained areas, plus one discovery that changed the
roadmap: **X.9 opened** (see below).

### 28.1 — the feed polled a failing backend 30×/min

`activityStore.ts` used `setInterval(poll, 2000)` unconditionally, so a backend
answering `500` was hit twice a second per open desktop. The loop is now a
`setTimeout` chain driven by a pure `nextPollDelay(failures)` (2 s → 4 → 8 → 16
→ 32 → 60 s cap). **The assertion that would have caught it** is a *count*:
E2E **13.9** mocks `/api/activity` to `500` and requires **≤ 3 requests in 7 s**
— a 2 s cadence cannot produce fewer than four.

### 28.2 — a failed job vanished instead of showing its outcome

The handler filtered on `matches!(s.state, Pending | Running)`, so a job that
failed one second ago disappeared on the next poll; the only trace was a bell
notification the user was not looking at. A `failed`/`cancelled` job now stays
for `FAILED_GRACE` (60 s after its last event, from a new
`ActivitySnapshot.updated_at`), the row carries a new `state` slug, and the
client renders it as a failure (no `role="progressbar"`, the localised word in
`--hv-error-text` at 9.4:1, and no dock badge — `count` now means *active*
rows while the dock/panel key off `activityRowCount`).
**The assertion that would have caught it**: `is_visible` is unit-tested across
all five states *and* both sides of the grace window, and
`tests/activity_integration.rs` asserts a fresh `fail_job` comes back with
`state: "failed"` and its reason in `detail`.

### 29.1 — `unpublish()` was dead code, so "discovery off" did nothing

There was no on/off control at all: `crate::mdns::unpublish()` had **zero
callers**, so nothing ever removed the Avahi service file or the `host-name`
override. The new admin setting (`GeneralSettings.mdns_enabled`, shipped on) is
applied to the host from the General save on `spawn_blocking`, and `unpublish`
now removes **both** artefacts (a new pure `remove_host_name`, the exact inverse
of `ensure_host_name`) and renames the daemon back; the reconciler skips its
tick while disabled. **The assertion that would have caught it** is a disk one,
not a status one: `tests/mdns_integration.rs` redirects `HV_AVAHI_CONF` /
`HV_AVAHI_SERVICE_FILE` into a tempdir and puts `sudo`/`which`/`systemctl`/
`busctl` shims first on `PATH` (the same seam `health_integration.rs` uses for a
slow `df`), then asserts `PUT {mdns_enabled:false}` **really deletes the file
and the `host-name` line** while leaving the rest of the config intact.

### 29.2 — a host-name change restarted Avahi and dropped every other advertisement

The roadmap's suggested fix ("a reload would do") is **wrong**:
`avahi-daemon(8)` states that `--reload` re-reads `/etc/avahi/services/*` and
**not** `avahi-daemon.conf`. The rename now goes through Avahi's D-Bus
`SetHostName` (`sudo busctl call`), **verified by reading `HostName` back** — a
call that returns 0 but leaves the old name must not be reported as success, or
the fallback never fires. A restart remains the fallback, logged at `warn`.
While in the module its five bare `Command` invocations were put under
`run_timed`/`run_timed_with_stdin` with three ordered ceilings and a source
guard.

### Discovery — X.9: X.2's "every subprocess is bounded" was an over-claim

Putting `mdns.rs` under the X.2 discipline immediately failed: the module still
had five bare invocations, and a sweep of the tree found **98 more in 26
files** (`sftp.rs` 21, `groups.rs` 14, `acl.rs` 8, `storage/database.rs` 8,
`restic.rs` 7, `setup.rs` 5, …). X.2's Done entry said *every*
`std::process::Command` of the tree was bounded; only the modules that received
a source guard were ever re-checked, which is precisely why the gap survived.
Tracked as **X.9** with the file-by-file count and "a source guard per module"
as acceptance.

### Counts

| Suite | Before → after | Added |
|---|---|---|
| `src/mdns.rs` (unit) | 10 → **18** | `remove_host_name` (override, commented override, no-op, exact inverse of `ensure_host_name`), the `enabled` flag round-trip, `status().enabled`, the `HV_AVAHI_*` path overrides, `every_subprocess_is_bounded` |
| `src/handlers/activity.rs` (unit) | 14 → **22** | `is_visible` (fresh failure, old failure, cancelled both sides, live always, completed never, no timestamp), `state_slug` ×5, the failed-row mapping, the aria2 row's state |
| `src/handlers/settings.rs` (unit) | 29 → **32** | the shipped default, the host-application source guard, the live-flag read |
| `src/settings_persistence.rs` (unit) | 15 → **15** | no new test — the new field rides the existing import/export round-trips |
| `tests/activity_integration.rs` | 11 → **13** | a fresh failure, a fresh cancellation, every row carries its slug |
| `tests/mdns_integration.rs` | 3 → **6** | the disable chain on disk, the enable chain on disk, admin-only + live read |
| `tests/settings_core_integration.rs` | 32 → **33** | the aggregate carries the live `mdns_enabled` |
| `tests/e2e/desktop/desktop.spec.ts` | 88 → **90** `test(` / 90 runtime | 13.9 backoff, 13.10 failed row |
| `tests/e2e/settings/settings.spec.ts` | 111 → **112** `test(` / 115 runtime | 3.7 the mDNS switch is staged and PUT in the General save; 3.4 re-targeted to `[data-testid="settings-autostart-row"]` because the card now carries two switches |

**Not verified.** Per instruction nothing was executed: `cargo check
--all-targets` is clean, `cargo test --no-run` compiles every target, and
`npm run check` reports 0 errors / 0 warnings. The disk assertions above are
derived from reading the code path (`apply_enabled` → `unpublish` → `sudo rm` /
`sudo tee` through the shims), not from a green run — `cargo test` is the
acceptance step. No frontend build was run (`npm run build`) because no asset
changed in a way `check` would not catch; run it before releasing.

## 2026-10-24 — Subprocess deadlines, round 2 (ROADMAP X.9)

X.2's Done entry claimed "every `std::process::Command` of the tree now runs
under `src/utils/process.rs`". It was only true for the modules that got a
**per-module source guard** — nothing ever re-swept the rest. Picking up item 29
found five bare invocations in `src/mdns.rs` alone, and a sweep found **98 more
across 26 files**.

### What was unbounded, and why it mattered

| File | Sites | The failure that was possible |
|---|---|---|
| `sftp.rs` | 21 | `getent`/`id`/`sudo useradd`/`chpasswd`/`mount --bind` on an unreachable export — the whole jail rebuild |
| `groups.rs` | 14 | `getent` behind a dead NSS/LDAP backend, `groupadd` on a locked `/etc/group` |
| `acl.rs` | 8 | `getfacl` on a directory served by a dead NFS server — the Explorer's ACL view |
| `storage/database.rs` | 8 | the bootstrap `getent`/`id`/`chown -R` before the listener is up |
| `restic.rs` | 7 | `which`/`version`, `mount`/`umount` of a snapshot for a restore |
| `setup.rs` | 5 | `visudo -c`, `which`, `chown` while holding the wizard's setup lock |
| `webdav/mod.rs` | 4 | the legacy-unit cleanup at startup |
| `auth/service.rs` | 3 | **the login path**: `python3` running PyPAM, `id -un`, `id -Gn` |
| `email.rs` | 3 | the MTA *spawn* (a wedged `sendmail`/`msmtp` hangs the fork) |
| `app_catalog/updates.rs` | 3 | `docker inspect`, `compose ps`, `compose pull` |
| `handlers/user_homes.rs` | 3 | `id`, `chown -R` on a home directory |
| + 15 files with 1–2 each | 22 | `ss`, `nvidia-smi`/`rocm-smi`, `ufw --version`, `df`, `findmnt`, `sudo kill`, `git`, `chpasswd`, `docker compose logs`, … |

### The test that would have caught it

`src/utils/process.rs::tests::every_subprocess_of_the_tree_is_bounded` walks
`src/`, finds every `Command::new(…)` and fails on one whose invocation
(`.output()` / `.status()` / `.spawn()` / `wait_with_output()`) is not wrapped by
`run_timed…`, a named bounded spawn helper, or an explicit
`tokio::time::timeout`. It prints the file and line. Its companion
`the_source_guard_detects_an_unbounded_call` proves it bites (a fabricated
unbounded line is rejected, a bounded one accepted), and
`every_exemption_names_a_real_file` keeps the exemption list from rotting.

Two families are legitimate exemptions, listed by name with a reason:

1. **Long-running children** — `clamscan`, `ddrescue`, `photorec`, the TestDisk
   scan. Their lifetime *is* the work; they are cancelled through their process
   group and carry `kill_on_drop`.
2. **Command builders** — `smart::std_command` / `tokio_command`,
   `encryption::run`. The bound lives at the call site, which the guard also
   scans.

### Counts

| Suite | Before → after |
|---|---|
| `src/utils/process.rs` (unit) | 13 → **16** |
| `src/acl.rs` (unit) | 0 → **1** |
| `src/groups.rs` (unit) | 9 → **10** |
| `src/sftp.rs` (unit) | 9 → **11** |
| `src/restic.rs` (unit) | 29 → **31** |
| `src/setup.rs` (unit) | 88 → **89** |
| `src/storage/database.rs` (unit) | 0 → **1** |

The other 20 touched files keep their counts: their fix is in the call sites, and
the per-module guard that would pin them lives in `utils/process.rs` today. A
per-module guard was added to the six files that had none.

**Not verified.** Per instruction nothing was run: `cargo check --all-targets` is
clean and `cargo test --no-run` compiles every target. The tree-wide guard was
also replayed as a standalone script over `src/` before being written, and it
reported exactly the two exemptions (`smart.rs`'s builder and the TestDisk
spawn) — i.e. the fixed tree is clean and the guard is not vacuous.

## 2026-10-24 — File Explorer residual hardening (ROADMAP 10.5, 10.6, 10.7)

The three leftovers item 10 deliberately left open. All three were real on a
normal path.

### 10.5 — the final path component was never resolved

Two holes, one root cause:

- `PathGuard::validate` returned **the caller's string** when the leaf did not
  exist yet ("a new file"), so `../` survived validation and the leaf was
  whatever was typed.
- `validate_path_access`'s **admin** branch returned `PathBuf::from(path)` for
  *every* admin path — so on that branch nothing was ever canonicalised, and a
  symlink planted between the check and the `open` was followed blind.

Both now canonicalise the parent and re-read the leaf with `symlink_metadata`
(a link resolving inside the roots becomes its target, a link resolving outside
is refused, a regular leaf that appeared meanwhile is resolved and re-checked,
an absent leaf comes back under its resolved parent). A `.`/`..` leaf is refused
outright. `PathGuard::resolve_final_component` serves the confined branch
(absolute **and** relative), `explorer::resolve_unconfined` the admin one: same
normalisation, no root confinement — an admin still browses anywhere.

**The assertion that would have caught it**: for a path whose leaf does not
exist, `validate` must return a value with **no `..` in it** and rooted at the
*resolved* parent — the old code returned the input, so the test fails on it
verbatim. Plus: a symlink leaf pointing outside the roots returns `None`, and the
admin branch still resolves `/etc/hostname` (it normalises, it does not
confine).

### 10.6 — a chunk was fully buffered

`/upload/chunk` took an `axum::body::Bytes`: the chunk was materialised in RAM
before the handler ran, and the only bound was the router-wide 10 GiB
`RequestBodyLimitLayer`. It now takes a stream and writes it frame by frame
(`stream_chunk_to_file`), so peak memory is one network buffer. Two caps are
computed **before** a byte is read — the session's remaining `total_size` and
`MAX_UPLOAD_CHUNK_BYTES` (32 MiB). Over either: `413` with the resume offset, the
temp file truncated back to its previous length, and the session offset **does
not move**.

**The assertion that would have caught it** is a shape one: a chunk delivered as
**several stream frames** must land byte for byte and advance the offset by the
total (a `Bytes` extractor cannot even accept that request), and an over-cap
chunk must leave the file's previous bytes intact and the offset unchanged.

### 10.7 — the share-stream permit was `unwrap()`ed

`SHARE_SEMAPHORE.acquire_owned().await.unwrap()` → a bare 500 (a task abort in
a debug build) if the semaphore were closed. It cannot happen with the static,
which is why it went unnoticed — and why an `unwrap` on a resource whose
lifecycle lives elsewhere is a latent panic. It now logs and answers `503`; the
4-stream ceiling is unchanged.

**The assertion**: a closed semaphore yields an error (never a panic), the
source no longer contains `acquire_owned().await.unwrap()`, and the ceiling still
refuses the fifth concurrent permit.

### Counts

| Suite | Before → after |
|---|---|
| `src/config/explorer_config.rs` | 4 → **9** |
| `src/handlers/explorer.rs` | 38 → **46** |
| `src/handlers/share_links.rs` | 0 → **2** |
| `tests/explorer_integration.rs` | 65 → **70** |

**Not verified.** Per instruction nothing was run: `cargo check --all-targets`
and `cargo test --no-run` are clean. The tests above were derived from the code
paths they pin, not from a green run — `cargo test --test explorer_integration`
and `cargo test --lib` are the acceptance step.

## 2026-10-24 — Virtual Machines and Transfer quotas (ROADMAP 32.1, 32.2, 33.1)

The last two residuals of already-closed items. All three were real on a normal
path.

### 32.1 — the screen relay outlived its socket for minutes

`screen_relay` piped bytes both ways with **no ping and no timeout**. A
half-open connection — a closed laptop lid, Wi-Fi that came back on a different
network, a suspended tab — left the relay task *and* the qemu VNC session alive
until the kernel's TCP retransmission gave up. It is now three tasks around one
shared `RelayLiveness` clock: both pumps `touch()` it (bytes, `Close`, and now
`Ping`/`Pong`), and a heartbeat pings every 30 s and closes **both** legs after
90 s of silence.

**The assertion that would have caught it**: the clock is a plain struct, so it
is unit-tested directly (`>=` at the deadline, `touch()` resets it), and the
interval/deadline constants are pinned to a ratio that allows three missed
pings. An end-to-end proof needs a browser that stops answering `Ping` and a VM
with a VNC display, which is a live-hardware pass — the unit tests pin the
mechanism, not the hardware.

### 32.2 — the ISO floor was checked after the bytes landed

The downloader streamed the whole body to disk and only then called
`finalise_iso`, so a link returning a multi-GB wrong file was fully written
before it was rejected, and a 2 KB HTML error page still cost a disk write to
discover. `classify_declared_length(Content-Length)` now decides **before** the
destination is created, and the same budget caps the running write (so a
chunked response and a lying server are both stopped mid-stream, file removed).

**The assertion that would have caught it** is the classification itself:
`Some(1)`, `Some(2048)` and `Some(MIN_ISO_BYTES - 1)` must all be `TooSmall` —
the old code had no such decision point at all — plus `Some(MAX_ISO_BYTES + 1)`
→ `TooLarge`, `None` → the cap as budget, and a source guard that the import
path no longer carries a `10_000_000` literal.

### 33.1 — one `id -nG` fork per quota check

`load_applicable` forked per check, and the resumable-upload path re-checks per
chunk: ~128 forks for a single 1 GB upload. Group membership now comes from a
process-wide cache keyed on the username with a `GROUP_MEMBERSHIP_TTL` of 5 s.
The Groups screen calls `invalidate_group_cache(username)` on a membership
change, so that path is immediate; the list an admin reads bypasses the cache so
it is never one TTL stale.

**The assertion that would have caught it**: a second lookup inside the TTL must
be served from the cache (a fresh entry is stored once and read back), the TTL
boundary must be exclusive, and the cache must be keyed per username — one
user's cached groups can never answer for another. Plus
`the_ttl_is_a_short_bound_that_is_actually_documented`, which asserts
`docs/guides/transfer-quotas.md` mentions the constant so the documented
staleness and the code cannot drift apart.

### Counts

| Suite | Before → after |
|---|---|
| `src/handlers/vm.rs` | 21 → **29** |
| `src/transfer_quota.rs` | 25 → **31** |
| `tests/vm_integration.rs` | 26 → **28** |

**Not verified.** Per instruction nothing was run: `cargo check --all-targets`
and `cargo test --no-run` are clean. The heartbeat and the download ceiling are
pinned at the mechanism level; demonstrating either end to end needs a live
Incus VM with a VNC display and a real HTTP peer, which is a separate
live-hardware pass.

## 2026-10-24 — Cloud Sync scheduler (ROADMAP 16.4)

`POST /rclone/jobs/{id}/run` was the only way a persisted job ever ran. The
`sync_jobs` row held the definition and nothing else — no due time, no cadence,
no last outcome — so a nightly sync could not be expressed, and the screen's
"last run / status" columns were the outcome of the click that had just
happened (hardcoded `idle`, then flipped from the response).

### What shipped

- **Migration 059**: `schedule_kind` (`none` | `interval` | `cron`),
  `interval_minutes`, `cron_expr`, `next_run_at` (indexed), `last_run_at`,
  `last_status`, `last_message` — all nullable, `none` as the default, so an
  existing appliance is unchanged until an admin opts in.
- **Validation before existence**: `NewSyncJob::validate_schedule` names the
  field (interval with no value / out of range, cron that cannot be parsed, cron
  over 120 chars). A job whose cadence cannot be computed must never exist, or
  the scheduler retries it on every tick — the failure mode item 12.3 had to fix
  on the tasks screen.
- **Due-time arithmetic**: an interval job is first due one interval *after
  creation*, and after a run is rescheduled from *the moment the run finished*
  (not from the previous due time — a 20-minute sync would otherwise re-fire at
  once and the overlap guard would skip it). A cron job is rescheduled from its
  own schedule, evaluated in the appliance timezone.
- **The loop**: `rclone::run_scheduler_loop`, spawned at startup, ticks every
  60 s, queries `due_now(now)` and **spawns** the run — the transfer leaves the
  HTTP request, paying off the request-shaped-transfer debt.
- **One core, one state write**: `execute_sync` split into `run_sync_core`
  (returns a plain `SyncOutcome`, so the scheduler can call it) + the HTTP
  shell; both paths go through `run_saved_job`, which writes `record_run`.
- **One run per job**: `JobRunGuard`, process-wide, so a click during a
  scheduled run does not start a second `rclone`.
- `PUT /rclone/jobs/{id}/schedule` (admin), and `POST .../run` now answers with
  the **updated job** so the screen renders the server's state.

### The assertions that would have caught it

| Defect | Assertion |
|---|---|
| a job never ran on its own | `due_now(now)` returns a job created with `interval_minutes: 60` **and** `next_run_at` computed at insert; a job with `schedule_kind: 'none'` is never returned |
| the screen's status was its own memory | `POST .../run` returns `last_status: "failed"` + `last_run_at` + a non-empty `last_message`, and `GET /jobs` reports the same row |
| a broken cadence lived in the table | create with `cron_expr: "definitely not a cron"` → `success: false`, the error names `cron_expr`, and the list is still empty |
| clearing a cadence left one last firing | moving to `none` makes `next_run_at` null **and** keeps `last_status` (history) |
| a scheduled run could delete | `request_for` always builds `delete: false`, `dry_run: false`, `update: true` |
| two entry points could double-run | `JobRunGuard` refuses the second claim, and releases on drop |

### Counts

| Suite | Before → after |
|---|---|
| `src/storage/sync_job_repo.rs` | 0 → **8** |
| `src/handlers/rclone.rs` | 39 → **43** |
| `tests/rclone_integration.rs` | 26 → **29** |

`frontend/src/routes/CloudSync.svelte` now reads `last_status` / `last_run_at` /
`next_run_at` from the server; `npm run check` is clean. The E2E suite was not
extended: the screen's geometry did not change, and a real scheduled run needs a
configured remote.

**Not verified.** Per instruction nothing was run: `cargo check --all-targets`,
`cargo test --no-run` and `npm run check` are clean. No test performs a real
`rclone` transfer or mount (the suite's long-standing rule), so the scheduler's
end-to-end behaviour is pinned at the state-machine level: due selection, the
recorded outcome, and the next due time.

## 2026-10-24 — Live-hardware activity poller (ROADMAP 28.3)

The last open *feature* of the roadmap. The activity taskbar could only report
work HomeVault had started: a row was a `PROGRESS_MANAGER` job some handler
created. Everything the host does on its own — an mdadm resync, a
`zpool`/`btrfs` scrub, a Syncthing folder still catching up — was invisible.

### What was added

`src/activity_poller.rs`, spawned by `web::run` (5 s tick), reads the live
sources and writes a synthetic job per operation:

| Source | Read | Bounded by |
|---|---|---|
| mdadm | `/proc/mdstat` (`tokio::fs`, no privilege) | n/a (one local read) |
| ZFS | `sudo zpool status <pool>` | `ZPOOL_PROBE_TIMEOUT` (10 s, `run_timed_async`) |
| btrfs | `sudo btrfs scrub status <mount>` | `BTRFS_PROBE_TIMEOUT` (10 s, `run_timed_async`) |
| Syncthing | `handlers::syncthing::folder_backlogs` (config + `db/status` per folder) | `HTTP_TIMEOUT` of the existing client |

### The three design decisions worth their tests

1. **One row per operation, not one per poll.** The job id is derived from the
   source (`sys:raid:md0:resync`, `sys:pool:tank:scrub`,
   `sys:syncthing:<folder>`), so a second tick *advances* the same row.
   **The assertion that would have caught the alternative**:
   `a_synthetic_job_is_created_then_advanced_on_the_same_id` asserts the
   percentage moves on one id and that `list_active_jobs` holds exactly one row
   for it after two ticks; `tests/activity_poller_integration.rs` asserts the
   same through `GET /api/activity`.
2. **The sweep can never touch a foreign job.** `plan_reconcile(known, live)` is
   pure and keeps only ids carrying `SYNTHETIC_PREFIX`. **The assertion**:
   `the_poller_never_touches_a_job_it_did_not_create` seeds `known` with a real
   backup id and requires `finished` to name only the `sys:` id.
3. **A missing source is a gap, never a fabricated value.** A missing
   `/proc/mdstat` is "no md arrays", a timed-out `zpool` contributes nothing,
   Syncthing is not probed without a `config.xml`. **The assertion**:
   `a_missing_mdstat_is_an_empty_feed_not_an_error` (empty feed, endpoint still
   `200`) and `an_unreadable_mdstat_is_an_empty_list_not_a_panic`.

### The parser bug this item existed to avoid

`/proc/mdstat` prints the percentage on a **continuation line**, not on the
`md0 : active raid1 …` header. A poller that read only header lines would report
every resync as idle — a "live" taskbar that stays empty while the disks grind.
`a_resync_on_a_continuation_line_is_reported_with_its_percentage` pins the
percentage end to end (`12.6` from the fixture to the row).

`zpool`/`btrfs` have the mirror-image trap: a **finished** scrub
(`scan: scrub repaired 0B in …`, `Status: finished`) must produce **no row** —
an activity row that never ends is worse than no row. Two tests per tool.

Syncthing is measured on **bytes**, not files: one 40 GB file out of ten
thousand is one file of the need, and a file-count ratio would read 99 % done.
`one_huge_file_out_of_many_is_not_reported_as_nearly_done` asserts the bar
stays under 97 %.

A **paused** Syncthing folder and a `DELAYED`/`PENDING` resync are not work in
progress and produce no row.

### Deliberately not covered

`luksFormat` — one-shot, no progress to read. A row could only say "running"
until it succeeded or failed, which the Storage screen already reports.

### Counts

| Suite | Before → after | Added |
|---|---|---|
| `src/activity_poller.rs` (unit) | 0 → **37** | mdstat (idle/resync/delayed/inactive/unreadable/attachment/keywords/ids), zpool (in progress/finished/young/resilver/block boundary), btrfs (running/finished+aborted/no percentage), Syncthing (bytes ratio, huge file, synced/empty/paused, label fallback), reconciliation (ended/foreign/still running/empty tick), `apply_plan` create+advance+complete, indeterminate storage, prefix/ids/job type, `every_subprocess_is_bounded` |
| `src/sse/progress.rs` (unit) | 26 → **28** | `emit_message_only` keeps a no-number job indeterminate across ticks, and does not erase a percentage it already reported |
| `src/i18n.rs` (unit) | 9 → **10** | the 3 new `desktop.activity_kind_*` keys exist and are non-empty in all 4 locales, and none renders a raw slug |
| `tests/activity_poller_integration.rs` | 0 → **10** | a resync is a live row with its %, a second tick advances the same row, a finished resync leaves the feed, a delayed resync is not a row, a missing mdstat is an empty feed not an error, a foreign job is never swept, synthetic ids are namespaced, an ended operation is planned for removal, no scrubbing pool produces no row, a synthetic row is hidden from a non-admin |
| `tests/e2e/desktop/desktop.spec.ts` | 90 → **91** `test(` / 91 runtime | 13.11 a synthetic `raid_resync` row renders with a localised name and its `%`, then leaves the dock when the operation ends |

i18n: 3 keys ×4 locales (`desktop.activity_kind_raid_resync`,
`activity_kind_pool_scrub`, `activity_kind_syncthing_sync`).

### The hardware pass — executed, and it found four defects

The first delivery of this item claimed "not verified". Running it found **four**
real defects, three of which would have made the feature silently useless in
production. All four are fixed, and each has a test that bites.

| # | Defect | What it would have done | Test that catches it |
|---|---|---|---|
| 1 | `apply_plan` created a job through `create_job()`, which mints a **uuid** | the row was inserted under an id nothing ever addressed again, so every following `emit_progress` was a silent no-op: **no synthetic row would ever have existed** | `a_synthetic_job_is_created_then_advanced_on_the_same_id` (and the `debug_assert` in `apply_plan` became meaningful) |
| 2 | `parse_md_percent` used `split_once('=')` on the whole line | the kernel draws the progress bar with `=` (`[==>…]`), so the split landed **inside the bar** and yielded `"=>…]  resync = 12.6"` — every resync reported an **indeterminate** bar | `a_resync_on_a_continuation_line_is_reported_with_its_percentage` |
| 3 | the zpool `%` scan zipped raw tokens and compared `"done"` literally | real output is `29.37% done,` — the comma meant the match never fired, so **every scrub** reported an indeterminate bar | `a_zpool_scrub_in_progress_is_reported_with_its_percentage` |
| 4 | `emit_message_only` wrote `0.0` into `last_event` | a row that had reached 29 % fell back to **"0 %"** on the next tick (the exact bug the method exists to prevent) | `emit_message_only_does_not_erase_a_known_percentage` |

Defect 1 is the most instructive: it is invisible to any test that only checks
that a plan is computed. Only a test that reads the **manager** back after
`apply_plan` could see it.

**What was run, and on what.**

- `cargo test --lib activity_poller` — **37/37 green**.
- `cargo test --lib sse::progress` — **28/28 green**.
- `cargo test --test activity_poller_integration` — **10/10 green**.
- `cargo test --lib` — **1698 passed, 22 failed**. The 22 are **pre-existing
  environment failures**, not regressions: `diff` against the same run on
  `HEAD` shows the fixed tree failing **only 9 fewer** tests than HEAD (31 → 22)
  and adding none. They are host-dependency tests (`smartctools`, `quota`,
  `zpool`, `ups`, `setup` grants) on a machine that lacks those binaries.
- **Live server**, real binary (`cargo build --bin homevault`, isolated
  `DATA_DIRECTORY`, `PORT=8097`, admin seeded via `POST /api/auth/setup`): the
  poller loop starts, polls the **real** `/proc/mdstat`, and
  `GET /api/activity` answers `{"success":true,"data":[]}` on three consecutive
  polls with **no warning, no error and no panic** in the log — the honest
  answer on a host whose `mdstat` lists no array. `401` without a session.
- **Real end-to-end pipeline**, against this machine's kernel:
  `tick()` driven by a synthetic `/proc/mdstat` produced
  `sys:raid:md0:resync progress=12.6` → advanced to `64.0` **on the same id** →
  and after the array left `/proc/mdstat`, `finished=["sys:raid:md0:resync"]`
  with an empty feed. That is the whole feature, verified outside the test
  harness.

**What could not be run here.** This host has `mdadm` and `btrfs` but **no
`zpool`** (no ZFS), **no Syncthing**, and **no active md array** — so no real
`zpool scrub`, no real btrfs scrub and no real folder backlog was observed. Those
arms are covered by parsers pinned to captured tool output and by the two
defects above, which the parser tests caught. A pass on a host with a scrubbing
pool and a paired device is still worth doing before an operator trusts those two
rows; it is no longer a blocker, and it is the only part of 28.3 not exercised
against live state.

## 2026-10-29 — Tranche T1: reachability and identity

T1 closes the "reachable from outside, and recoverable when I am locked out"
half of the roadmap. Four features shipped together, each with its own seam so
that no test needs the network, a real CA or a real mailbox.

### What was added

- **Certificate automation** (`src/acme.rs`, `src/acme/client.rs`,
  `src/acme_worker.rs`, `src/handlers/acme.rs`) — an in-house RFC 8555 client
  (`rcgen` P-256 account key, `reqwest` transport). The `Transport` trait is
  the seam: every test drives a **scripted CA** and never opens a socket.
  Statuses are `disabled|idle|pending|valid|expiring|error`; renewal fires at
  `RENEW_WHEN_DAYS_LEFT = 30`. A definitive refusal (bad domain, unauthorized
  challenge) sets `consecutive_failures = u32::MAX`, which parks the worker
  until an admin changes the domain, challenge, provider or directory URL —
  the settings handler is what resets the counter.
- **Dynamic DNS** (`src/ddns.rs`, `src/handlers/ddns.rs`) — providers
  `duckdns|dynv6|cloudflare|custom`, a 5-minute worker that only calls out
  when the address actually changed, and a public-IP probe that uses a UDP
  `connect` to `1.1.1.1:53` (it reads the local socket address; no packet is
  sent).
- **Reachability advisor** (`src/reachability.rs`,
  `src/handlers/reachability.rs`) — a read-only checklist (`tls.enabled`,
  `tls.trusted_certificate`, `acme.automation`, `ddns.enabled`, `names.agree`,
  `ports.https`, `ports.http01`, `tls.mode`) that explains *why* the appliance
  is not reachable instead of only reporting that it is not.
- **Identity** (`src/identity.rs`, `src/handlers/identity.rs`, migration
  `060_identity.sql`) — trusted devices (a `hv_trusted_device` cookie,
  `HttpOnly` + `SameSite=Strict`, 30 days, stored as a SHA-256 hash) and a
  rescue address (six-digit codes, 15-minute TTL, 5 attempts, salted hash).
  A correct rescue code is exchanged for a fresh **offline recovery code**, so
  the existing reset screen stays the single password-reset path — one policy,
  one attempt limit. Both public routes (`/auth/recover`,
  `/auth/recover/verify`) answer identically whether the account exists, has a
  rescue address, or was throttled.

Supporting pieces: `src/outbound.rs` (one place for outbound HTTP, so a future
proxy setting has a single home), `TlsMode::Acme` in `src/tls.rs` (resolves to
the issued pair, falls back to the self-signed certificate with a warning, and
`validate_settings()` refuses `acme` unless `acme::automation_active()`),
notification events `certificate.issued` / `certificate.renewal_failed`, and
`settings_persistence::reset` clearing both new stores.

### Tests

| Suite | Count | What it pins |
|---|---|---|
| `src/acme.rs` unit | 29 | settings round-trip and validation, status derivation, the `RENEW_WHEN_DAYS_LEFT` boundary, the parked-after-refusal rule, the 1 MB SHA-256 vector (`zcduXJkU-5KBocfihNc-Z_GAmkiklyAOBG05zMcRLNA`), challenge tokens and key-file permissions |
| `src/acme/client.rs` unit | 12 | JWS/nonce handling, order + authorization parsing, the two challenge kinds, error-object mapping — all against a scripted transport |
| `src/ddns.rs` unit | 17 | provider request shapes, `{ip}` substitution + HTTPS-only custom URLs, address-change gating, the no-traffic IP probe |
| `src/reachability.rs` unit | 13 | every check's pass/fail/skip and the summary ordering |
| `src/identity.rs` unit | 8 | hash irreversibility, the six-digit code, the account-and-address salt, device labels, expiry comparison |
| `src/handlers/{acme,ddns,identity,reachability}.rs` unit | 3 + 2 + 6 + 1 | cookie shape/flags, mail-body wording, throttling, the handler-level seams |
| `tests/acme_integration.rs` | 10 | the full issuance walk over the scripted CA: account → order → challenge → finalize → download; a refused challenge parking the worker; renewal; the settings API refusing `acme` mode while disarmed |
| `tests/ddns_integration.rs` | 7 | settings persistence, the custom-URL refusal, a no-op update when the address is unchanged |
| `tests/identity_integration.rs` | 9 | device listing / revoking one / revoking all; refusing another account's device; an address *not* stored until the code comes back; a wrong code failing; a trusted device skipping the second factor while another account's does not; a wrong password never reaching the device check; recovery answering the same for every case |
| `tests/e2e/settings/acme.spec.ts` | 5 | the card's controls, the status badge being a documented value, a copied URL refused without storing, the DNS fields appearing only for `dns-01` |
| `tests/e2e/settings/ddns.spec.ts` | 7 | provider switching, the custom-URL rules, the own-save button, a refused save leaving the stored config untouched |
| `tests/e2e/account/identity.spec.ts` | 17 | the empty trusted-device state, "revoke every device" disabled while nothing is trusted, the rescue form, a malformed address refused, an unknown account getting the generic recovery answer, a wrong code keeping the user signed out, the opt-in switch setting an `HttpOnly`/`SameSite=Strict` cookie |

`cargo test --lib` **1817 passed, 0 failed**; the whole tree (`cargo test`)
is **2 893 passed, 0 failed** across 63 integration suites plus the lib and
doc tests. The three new integration suites are green (10/7/9). `cargo test
--test acme_integration --test ddns_integration --test identity_integration`
was the acceptance run for T1.
`cargo clippy --all-targets` reports no warning in any T1 file (the 65
remaining ones are all pre-existing in untouched modules — the same set HEAD
produces); `cargo fmt --check` is clean on every file this pass touched;
`npm run check` 0 errors and `npm run build` OK.

### What is deliberately not covered

- **No live ACME run.** Issuance is proven against the scripted CA only. A
  real Let's Encrypt staging run needs a public name, an open `http-01` port
  or a DNS provider token, and an internet connection; none of that belongs in
  `cargo test`.
- **Passkeys / WebAuthn were deferred**: the `webauthn` crate is not in the
  offline vendor set, and adding a dependency to ship a half-tested
  authenticator flow would be worse than not shipping it.
- **No real mail is sent by any test.** The rescue flows are asserted up to
  the `EmailService` call; delivery is the existing, already-tested email
  path.
- The E2E specs need a live, seeded server (`TESTING.md` "Playwright" section)
  and were not run in this pass.

### One pre-existing flake fixed on the way

`activity_poller::tests::a_known_percentage_survives_a_tick_without_one` and
`a_synthetic_job_is_created_then_advanced_on_the_same_id` both drove the
process-wide `PROGRESS_MANAGER` with the *same* derived id (`sys:scrub:tank`),
so the harness could interleave them on two threads and the second test's
`remove_job` made the first one's row vanish. The whole-tree run failed on it
once out of three, at random. The continuity test now uses its own pool name
(`tank-pct-continuity`); `cargo test --lib` went back to green on three
consecutive runs. No production code changed.
## 2026-10-29 — Explorer instant copy (reflink): QW-03

A copy on a copy-on-write filesystem should share the source's extents instead
of reading and rewriting every byte. This pass puts a three-step cascade in
front of the verified stream and makes the strategy **observable**, because a
speed-up nobody can see or audit is indistinguishable from a bug.

### What was added

- **The cascade** (`src/browser/reflink.rs`, 19 unit tests) —
  `copy_file_cascade(src, dst, mode, stream)` tries `ioctl(FICLONE)`, then
  `copy_file_range`, then the caller's verified stream, and returns
  `Invocation { method, bytes }`. `CopyMethod::{Clone, KernelClone, KernelCopy,
  Stream}` carries `slug()` (the wire value), `rank()` (the cascade position)
  and `is_instant()`. `KernelClone` is **reserved**: nothing produces it today,
  because no syscall can prove a kernel-side copy shared extents.
  `copy_file_cascade` is generic over the error type (`E: From<std::io::Error>`)
  so the stream closure keeps `BrowserError::IntegrityMismatch` intact instead
  of flattening it into an `io::Error`.
- **Refusal vs failure is a policy, not an accident** —
  `is_unsupported_errno` accepts `EXDEV`, `EOPNOTSUPP`, `ENOTTY`, `EINVAL`,
  `EBADF` (this pair cannot share extents → fall back) and rejects `EIO`,
  `ENOSPC`, `EPERM`, `EROFS` (real → fail). A failing disk is never hidden
  behind a slow copy that happens to succeed.
- **The atomic contract is untouched** — `FileBrowser::copy_file_instant`
  still writes a `.hvtmp-<uuid>` sibling, applies the source's permissions /
  ownership / mtime *before* the swap, then `rename`s and `fsync`s the parent.
  One walk (`copy_dir_inner`) backs `copy_dir_all`,
  `copy_dir_all_with_mode`, `copy_dir_all_tracked` and
  `copy_dir_all_tracked_with_mode`; `move_item_with_mode` returns
  `Option<Invocation>` — `None` for a same-filesystem atomic rename.
- **The setting** (`InstantCopyMode::{Auto, Off}`, default `Auto`) — the
  singleton `instant_copy_policy` row (migration `062_instant_copy.sql`), read
  and written through `reflink::InstantCopyService`. `parse` is lenient (an
  old/corrupt stored row must never break copying) while `parse_strict` guards
  the request path (telling an admin a typo was saved would leave the appliance
  on a policy they never picked). The mode is read **once per operation**, so a
  single copy can never mix two policies.
- **Mount evidence** (`describe_mounts` / `ReflinkMount`) — from
  `/proc/self/mounts`, longest-prefix mount, octal escapes decoded. Btrfs yes,
  XFS only with `reflink=1`, **ZFS never claimed** (pool-level block cloning is
  invisible to the mount table, so the ioctl stays the authority). The list is
  information for the modal, never a gate.
- **Honest reporting** — `reflink::CopyStats` is one `AtomicU64` per method,
  indexed by `rank()`, recorded from the copy thread and summarised by the job;
  `summarize_methods` folds a tree to its **slowest** method. So
  `tracked_completion_message` says "Copied N file(s) instantly (no data
  copied)" only when every file was shared, "N cloned, M copied" for a mixed
  tree, and never the word "instant" for a kernel copy. A single-file copy
  answers `method`/`instant`/`bytes`; a same-filesystem move answers
  `method: null`.
- **API** — `GET|PUT /explorer/instant-copy` (read any signed-in, write admin)
  in `src/web/mod.rs`; `instant_copy_mode(&state)` is the single reader, and
  `BLOCKING_TOKENS` grew 19 → 20 with `FileBrowser::copy_file_instant` so the
  source guard still catches a blocking call moved into an `async fn`.
- **Frontend** — an admin-only toolbar button (`explorer-instant-copy-btn`)
  opening the "Instant copy" modal (radio group `explorer-instant-copy-auto` /
  `-off`, mounts list, error line), a `copied_instantly` toast in
  `copySelectedTo`, and `getInstantCopySettings` / `setInstantCopyMode` in
  `api.ts`. 15 keys in the four locales.

### Tests

| Suite | Count | What it pins |
|---|---|---|
| `src/browser/reflink.rs` unit | 19 | cascade method + byte contract (`clone` where the filesystem shares extents, `kernel_copy` when `FICLONE` is refused and the range copy fills the placeholder, `stream` under `mode=off`, empty file, missing source as a real error, "never claims a clone it did not get"); `CopyMethod` slugs/ranks/instantness; `CopyStats` (counter per rank, `slowest` fold, `all_instant`); `summarize_methods`; lenient vs strict mode parsing; the errno matrix; `/proc/self/mounts` parsing (escapes, longest prefix, a sibling path not confused, btrfs/xfs-reflink/zfs); `describe_mounts` (dedupe, `/` fallback) |
| `src/handlers/explorer.rs` unit | 4 new (50 total) | the completion message reports only what happened (empty / all-instant / mixed / verb follows the operation), a kernel copy is never called instant, the request parse rejects what the stored parse coerces |
| `tests/explorer_integration.rs` | 7 new (82 total, **82/82 green**) | single-file copy reports a strategy with `instant` iff it is one, plus its byte count; `mode=off` forces `stream`; a directory copy still returns a tracked job under both modes; `GET` any signed-in / `PUT` admin-only; an unknown mode refused with nothing stored; same-filesystem move reports `method: null` |
| `tests/e2e/explorer/explorer.spec.ts` §17 | 5 new (69 `test(`, 71 runtime per project) | 17.1 strategy + exact length, with `instant` checked against both the slug and the server's own mount evidence (so it holds on ext4 and on Btrfs), 17.2 `mode=off` → `stream` then restore, 17.3 the toolbar opens the modal, both radios and the mounts render, each choice is persisted server-side, 17.4 an unknown mode refused with nothing stored and a non-admin `PUT` is `403`, 17.5 a same-filesystem move reports `method: null` — **5/5 green live 2026-10-30** |

`cargo test --lib` **1865 passed, 0 failed**; `cargo test --test
explorer_integration` **82/82**. `cargo build --lib` clean, `npm run check` 0
errors / 0 warnings, `npm run build` OK. E2E §17 **5/5 live**; the whole file
**71/71** after the 2026-10-30 reliability fixes (RF-01 design-system leak,
RF-02 admin-delete scratch exemption, RF-03 sidebar-collapse timing).

### What is deliberately not covered

- **A real clone is never asserted in CI.** The dev host's `/tmp` is ext4, so
  `FICLONE` answers `EOPNOTSUPP` there. Every test therefore asserts
  `inv.method.is_instant() == reflink::is_reflink_capable(&src)` (or
  `method == "stream"` under `mode=off`) — pinning a literal `Clone` would pass
  on a Btrfs box and fail everywhere else, and the reverse would hide a real
  regression on the filesystems the feature exists for. The E2E 17.1 derives
  the same expectation from the mount evidence `GET /explorer/instant-copy`
  itself returns, so one spec is true on both kinds of host.
- **No loop-mounted Btrfs / XFS in `cargo test`.** It needs root and a spare
  block device; the cascade's decision logic is pure and covered, and the
  acceptance criterion ("on Btrfs free space is unchanged") is a live check.
- **Section 17 was run live on 2026-10-30 against a scratch server**
  (`PORT=8099`, `DATA_DIRECTORY=/tmp/opencode/hv-e2e-data`, `e2eadmin`
  seeded): **17.1–17.5 all pass** (Firefox). The two defects that run found
  are fixed:
  1. **17.3 — the policy modal was 1246 px tall in a 720 px viewport**, so
     the `auto` radio sat above the viewport and `.check()` failed with
     "outside of the viewport". Cause was a design-system leak:
     `base.css`'s `input { width:100%; border; padding }` (meant for text
     fields) also applied to `<input type="radio">`, stretching each radio to
     the full 374 px label width and forcing the option text to wrap one word
     per line. Fixed by giving both radios the same `h-4 w-4 flex-none
     accent-[var(--hv-primary)]` the other screens already use, plus
     `max-h-64 overflow-y-auto` on the mounts list so a host with many
     filesystems scrolls instead of pushing the radios out of the dialog.
     Measured after the fix: dialog **580 px**, radio **16×16**, in-viewport.
  2. **`openWork` clicked a stale favorite** — every failing run reported
     `currentPath` equal to a *previous* run's `hv-fx-e2e-*` dir. `DELETE
     /api/explorer/favorites` is per-path, and neither `openWork` nor
     `afterAll` removed `e2ework` entries pointing into **old** work dirs, so
     they accumulated one row per run and the sidebar's `.first()` matched the
     oldest. Both now sweep every favorite under `hv-fx-e2e-*`
     (`staleWorkFavorites()`), so a filtered `-g "17 …"` run — which skips
     describe 15's own cleanup — no longer poisons the next one.
  That first full-file run was **69/71**. Both reds were then diagnosed and
  **fixed at the cause**, and the file is now **71/71**:
  - **13.1** ended on `POST /explorer/delete` of its own work dir.
    `handle_explorer_delete` was the only admin path still calling
    `state.path_guard.validate` directly (every sibling uses
    `validate_path_access`, unconfined for admins), so an admin could create,
    fill and copy into a folder outside the roots but never remove it. Fixed
    with a narrow scratch-area exemption — outside the roots stays refused
    except under `$TMPDIR`/`/tmp`/`/var/tmp` (`is_under_temp_root`), with the
    browsing-root guard still winning. Covered by
    `test_admin_delete_allows_scratch_outside_roots_only` (+1 integration,
    **82/82**) and 2 new `is_under_temp_root` unit tests (**52** in the
    module). ROADMAP 10.4's doctrine test is unchanged and still green.
  - **1.6** sampled `boundingBox()` immediately after `toHaveClass`, i.e. on
    the first frame of `WindowSidebar`'s 180 ms `transition: width`, and
    intermittently measured 72 px instead of 64 px. Both width assertions now
    `expect.poll`, so the box is read once it settled. This was the
    order-dependent flake the Explorer row had been carrying.
- **Not covered live:** an actual `clone` — `/tmp` on the dev host is ext4, so
  `FICLONE` answers `EOPNOTSUPP` and the copy lands on `kernel_copy`. The
  cascade's clone arm is covered by the unit tests, and 17.1 derives its
  expectation from the server's own mount evidence rather than asserting
  `clone`, so it passes on both kinds of host.

### Deviation from the proposal

`cp --reflink=auto` under `spawn_blocking` was the suggested shortcut and was
rejected: it silently falls back to a plain copy and returns success either
way, so it can neither prove it cloned nor report which strategy ran — the one
thing this feature has to be honest about. The `ioctl` is issued in-process
with `O_CLOEXEC` descriptors and no subprocess at all, so there is also no
deadline to enforce.

## 2026-10-31 — Share links: password, download cap, one-time link (QW-04)

| Suite | Count | What it pins |
|---|---|---|
| `src/handlers/share_links.rs` unit | **2 → 11** | exhaustion arithmetic (an uncapped link is never spent, the N-th download is the last, `max_downloads = 1` dies after one completed download); a token is `[A-Za-z0-9_-]{1,128}` and nothing else (a crafted path is refused before it is echoed into a header/cookie `Path`); the unlock cookie is `HttpOnly`, `SameSite=Lax`, path-scoped to `/share/<token>`, 15 min, `Secure` only over TLS; the cookie is parsed out of a `Cookie` header among others and a forged/expired/other-link nonce never unlocks; revoking drops both the unlock and the failure count; the password budget is 8 failures per 5-minute window, per link, cleared by a correct password |
| `tests/share_links_integration.rs` | **13 → 21 (21/21 green)** | the cap counts completed transfers only (2 allowed, 3rd `400`, list shows `2/2` + `exhausted`); a one-time link is dead after one completed download; an **aborted** download (reserved then dropped) does not spend it; **a completed download counts when hyper frames the body** (read exactly `Content-Length` bytes, then drop the stream — the shape `to_bytes` cannot reproduce, and the one that used to report every real download as interrupted); `HEAD` is slot-free, carries `x-hv-share-auth: password`, and honestly reports a spent link as `400`; the unlock flow (wrong → `401` + reason, right → `200` + `Set-Cookie`, cookie → `200`, forged nonce → `401`, `?password=` → `401`) and its `Cache-Control: no-store`; 8 wrong passwords then `429` even with the right one, while 20 empty probes cost nothing; `POST /share/{token}/unlock` reachable through the real `web::create_app`; `password_hash`/`active_downloads` never serialised |
| `tests/e2e/explorer/explorer.spec.ts` §16 | **+2 `test(` (16.4, 16.5); §16 5/5 green** | 16.4 — create a password + one-download link in the modal, open it in a session-less browser context: password field shown, wrong password → alert with no password in the URL, right password → a real download, `HEAD` polls to `400`, a fresh visit shows the unavailable state and **no** password field; 16.5 — an open link downloads straight away and never renders a page. **Live 2026-10-31 on Firefox and Chromium** (scratch server `:8099`, `e2eadmin` seeded via `POST /api/auth/setup`), alongside 16.1/16.2/16.3 |

`cargo test --lib` **1874/1874**, `cargo test --lib handlers::share_links`
**11/11**, `cargo test --test share_links_integration` **21/21**, `cargo test
--lib i18n` **11/11** (new keys at parity in en/fr/de/es), `npm run check` 0
errors / 0 warnings, `npm run build` OK, E2E §16 **5/5 Firefox + the new three
on Chromium**.

**What the pass fixed beyond the feature itself** (all five were in the
in-flight implementation, each one would have shipped):

1. **A spent link still answered `HEAD` `200`**, so the public page promised a
   download it then refused. `is_exhausted` is now checked before the probe
   branch — the page can say "limit reached" before asking for a password.
2. **A failure after the reservation leaked the slot forever** (unopenable file,
   unstatable file, unsatisfiable `Range`, closed stream pool). A capped link
   would silently die. Every such return now releases the reservation.
3. **A client disconnecting mid-stream leaked it too** — the wrapper only
   released on `Poll::Ready(None)`. `CompletionShareStream` now releases in
   `Drop` (guarded by `Handle::try_current`, so dropping a response outside a
   runtime cannot panic), which is what makes the "aborted download does not
   spend a one-time link" test possible.
4. **A completed download was recorded as interrupted, so a one-time link was
   never spent.** Found by the live E2E run, not by the tests: hyper stops
   polling a sized body once it has written `Content-Length` and *drops* the
   stream, so `Poll::Ready(None)` never happens on a real download —
   `axum::body::to_bytes` in the integration tests polls to the end and hid it.
   Completion now follows the byte count, evaluated in `Drop` as well, and
   `test_share_link_completed_download_counts_when_hyper_frames_the_body`
   reproduces hyper's framing (read exactly `Content-Length` bytes, drop).
5. **The service worker cached `/share/` cache-first**, so a spent one-time
   link (or a revoked one) was replayed from the browser's cache with no
   network request at all — also found live. `sw.js` now skips `/share/`, and
   the file response carries `Cache-Control: no-store`.

Also: `/share/{token}` is both the download and the landing page, so a browser
navigation (`Accept: text/html`) that cannot be answered with the file now gets
the page — the password form, or the reason the link is unavailable — instead
of JSON in the address bar, while download managers and `fetch` (which sends
`Accept: */*`) still get the file or the status. `signed_in` links additionally
accept the `hv_session` login cookie, because a top-level navigation cannot
send `x-session-token` — verified live (anonymous → page, signed-in cookie →
the bytes, anonymous API → `401`).

**Deviation from the proposal**: the unlock cookie is short-lived (15 min)
rather than *single-use*. A video seek or a resumed download re-requests with
`Range`; a single-use cookie would break the transfer halfway, and the security
property that matters (the password never in a URL, and an unlock that expires)
is unchanged.

`src/storage/share_link_repo.rs` still carries the repo-level unit tests
(`generate_share_token` shape, `compute_expiry`), unchanged.

## 2026-10-31 — Remote syslog forwarding: QW-08

Written in this pass; **the Rust suites and the E2E spec were NOT executed**
(explicit instruction for this pass). What each one pins:

| Suite | Count | What it pins |
|---|---|---|
| `src/syslog_forward.rs` unit | **23** | exact RFC 5424 and RFC 3164 bytes (the RFC's own example shapes); `PRI = facility × 8 + severity` (user.info 14, kern.err 3, auth.warning 36, local7.debug 191); a missing header field becomes `-`, never an empty string (which would shift every following field); a space in a header field becomes `_`; RFC 3164 day padding (`Oct  3` vs `Oct 14`) and UTC second-precision RFC 5424 stamps; the configured format actually reaches the wire; UDP truncation at 1024 octets **on a character boundary** (a multi-byte character is never split) while a stream transport may carry 8 KiB; strict vs lenient parse of protocol/format (a bad stored value falls back, a bad *request* is refused); the host/port/facility matrix (URL, path, bracketed IPv6, leading hyphen, 254-char name all refused); a half-filled **disabled** entry stays saveable; TLS requires a CA and the CA must parse as PEM while a private key does not count; `normalize` follows `enabled`; the queue drops **oldest-first**, reports the drop, keeps batch order and bounds a batch; the status rules; and a scripted transport (one payload per event, empty batch sends nothing, the collector's refusal is reported verbatim) |
| `tests/syslog_forward_integration.rs` | **11** | admin-only matrix on all three verbs (no token 401, member 403, admin 200); save → reload round-trip from disk; nine invalid payloads refused with **nothing stored**; a disabled entry saveable with an empty destination; `status`/`last_error`/`last_sent_at` not writable (a fabricated `forwarding` stays `idle`); a **real localhost UDP collector** receives the framed test message (`<14>1 … homevault … test …`); a **real localhost TCP collector** receives the newline-framed RFC 3164 line; an unreachable collector is reported by the test button **without** failing the save; the test button refused while disabled; `events::log_event` queues exactly one event while enabled and none while disabled; and `update_observation` recording a failed batch **without reverting** a destination the admin saved meanwhile (the worker must never write a stale snapshot) |
| `tests/e2e/settings/syslog.spec.ts` | **16 `test(`** | card shape + both dropdowns; the status pill carries a documented `data-state` **and** a translated label (not colour alone); queue counters re-read from the API; the CA field appears only for TLS; a save round-trip and enable → `idle` → disable → `disabled`; a URL-as-host refused with the server's own reason on screen; TLS-without-CA refused; a fabricated `status` ignored; the test button's success (`role="status"`, "check the collector") and refusal (`role="alert"`, the address quoted) paths with `page.route` (no packet leaves the machine); the button disabled while off; a member refused on all three verbs; labels + `aria-labelledby` on both switches; no duplicate ids; no horizontal overflow with the CA textarea open — `afterAll` leaves forwarding **disabled** |

Compilation was verified (`cargo check --all-targets` clean, `npm run check` 0
errors / 0 warnings, `npm run build` OK); no test was run.

**Design notes worth keeping** (each one is a decision a future change could
undo):

1. **`events::log_event` is the only forwarding call site**, so a new audited
   mutation is forwarded without knowing the feature exists. It reads a cached
   `enabled` flag (1 s TTL, invalidated on every write), so the disabled path
   costs no file read.
2. **Drop-oldest, drop-on-failure, never retry.** Replaying an audit stream
   hours out of order is worse than a gap; the counters and `last_error` say
   what was lost. This is why the queue is a `VecDeque` under one mutex rather
   than an `mpsc` (an `mpsc` can only drop the *newest*).
3. **TLS loads no system roots.** Trusting every public CA would make "pinned
   CA" decorative for a collector with a self-signed local name.
4. **Host journal sources are not forwarded.** They belong to the OS, would
   duplicate on the collector, and are not bounded by this queue.
5. **`status` is a server-owned observation.** A client cannot write it, which
   is why the request struct is a flat optional subset instead of
   `SyslogSettings` (`SyslogCard.svelte` sends only what it edits).

## 2026-10-31 — Wake-on-LAN: QW-09

Written in this pass; **the Rust suites and the E2E spec were NOT executed**
(explicit instruction for this pass). Compilation was verified:
`cargo check --all-targets` clean, `npm run check` 0 errors / 0 warnings,
`npm run build` OK.

| Suite | Count | What it pins |
|---|---|---|
| `src/wol.rs` unit | **17** | the packet's exact shape (6 × `0xFF` then the MAC sixteen times, 102 bytes, and byte 6 is the MAC rather than a seventh `0xFF` — a 108-byte packet is silently ignored by some NICs); the accepted MAC spellings (`AA:BB:…`, `aa-bb-…`, `aabb.ccdd.eeff`, bare hex, with surrounding spaces) and the refused ones (wrong length, non-hex, all-zero, `FF:FF:FF:FF:FF:FF`, a multicast `01:…`), with a locally-administered `02:…` accepted; the netmask↔prefix round trip including a **non-contiguous** mask (`255.0.255.0`) that must not be rounded into a subnet; directed broadcast for `/24`, `/8`, `/32` (none) and `/0`; `same_subnet`; `ipv4_interfaces` dropping loopback and addressless rows while keeping the directed broadcast; interface selection by target subnet, by "first up", and never a down interface; the destination preference (override → directed broadcast → limited broadcast) with a **named** override refused (it would be a DNS lookup between a click and a packet); label and destination validation; the report's wording ("only confirms it left", never "woken"); the `wake` pre-step parser (`deny_unknown_fields`, default delay, `host_id: 0` refused, 601 s refused, 600 s accepted, absent block = an ordinary task) |
| `tests/wol_integration.rs` | **7** | the admin-only matrix on all five routes (no token 401, member 403, admin 200 with the interface evidence); the CRUD round trip (label trimmed, MAC canonical, duplicate name refused with the reason, second delete a miss); ten invalid hosts refused **with nothing stored**; a **real localhost UDP socket** receiving the 102 bytes (6 × `FF` + 16 × MAC, byte for byte); the wake endpoint reporting `sent`, the destination, both ports, no interface when the admin chose the destination, and the "not confirmed" sentence; an ad-hoc MAC accepted and a bad one / a missing selector / a vanished host refused; and the wake-block gate at task-creation time |
| `tests/e2e/settings/wol.spec.ts` | **13 `test(`** | card shape (form, list, interface evidence); the interface list naming a `→ broadcast` or explaining that no interface has an IPv4 address; the empty state's next step; add → list → edit → delete against the API; a bad MAC and a **named** destination refused with the server's reason as `role="alert"`; a duplicate name refused; the wake answer (`role="status"`, "only confirms it left", never "woken"); a refused wake as `role="alert"`; a member refused on read **and** wake; labels for all three fields plus accessible names on the icon-only delete buttons; no duplicate ids; no horizontal overflow. `POST /api/power/wol/wake` is **mocked** (no packet leaves the machine) and `afterAll` sweeps every `e2e-wol*` row |

**Design notes worth keeping** (each is a decision a later change could undo):

1. **Never "woken".** The report, the card and the run output all say the packet
   *left*. A magic packet has no acknowledgement and the target is off by
   definition, so "woken" would be an unverifiable claim in the exact case the
   feature exists for.
2. **Directed broadcast, not `255.255.255.255`.** The frame is a link-layer
   broadcast; addressing the selected interface's directed broadcast is what
   makes the kernel route it out of *that* interface, which is also what lets
   the report name the interface honestly.
3. **No subprocess.** The interface inventory is the Network screen's existing
   `/sys/class/net` read, so opening the card or waking a host forks nothing
   (and a WOL action cannot block on `ip`).
4. **Refuse a multicast MAC.** `FF:FF:FF:FF:FF:FF` wakes every machine on the
   segment; `01:…` is not a NIC. Both are typos, not intentions.
5. **A wake pre-step fails the run.** The one outcome the option exists to
   prevent is a task (a backup, a scrub) running against a machine that never
   came up; silently continuing would hide it until the data was missing.
6. **`wake` is a pre-step, not a task type.** Every task type may carry it, so
   it lives beside `TaskAction` in the payload and has its own validator rather
   than becoming a fifth variant with four combinations.

Two defects were fixed while re-reading the new screen and its spec, before any
run (both would have surfaced as reds and one was a real accessibility bug):

- **`WolCard.svelte` gave `#wol-label` two `<label>` elements** — the form's row
  heading and the field itself. Two labels concatenate into one accessible name
  ("Add a machine Name"), so a screen reader announced the heading as part of
  the field. The heading is now a `<span id="wol-form-heading">`; 5.2's
  `label[for="wol-label"] === 1` stays the guard.
- **`syslog.spec.ts` 1.2 asserted the status pill's label *differs* from its
  `data-state`** — false by design in English ("Disabled" labels `disabled`).
  It now asserts what WCAG 1.4.1 actually requires: the pill renders words, and
  each state carries its own class (`syslog-badge-<state>`), so colour is never
  the only carrier.

## 2026-10-31 — Login auto-block and the request-rate row: QW-05

Written in this pass; **the Rust suites and the E2E spec were NOT executed**
(explicit instruction for this pass). Compilation was verified:
`cargo check --all-targets` clean, `npm run check` 0 errors / 0 warnings,
`npm run build` OK.

| Suite | Count | What it pins |
|---|---|---|
| `src/auto_block.rs` unit | **16** | the threshold is inclusive (`>=`, so `threshold = 5` bans on the fifth failure); a disabled policy never bans whatever the count; the ban window's two RFC 3339 stamps; the protected-address matrix (`127.0.0.1`, `127.0.0.53`, `::1`, `0.0.0.0`, `::`, `::ffff:127.0.0.1`) while a LAN/public address may be banned; empty/`unknown`/`1.2.3.1/24` are never banned (a rule built from them would be forged); "the caller never bans its own address" (`must_not_ban`, including the trimmed form); the shipped defaults (off, 5/300 → 1800 s, no firewall rule, DoS on at 600/min); a disabled policy may keep impossible numbers; seven field-by-field refusals of an enabled policy plus a zero DoS cap being fine when the row is off; `clamped` bringing a hand-edited row into range; `is_active` false exactly at the expiry; remaining seconds/minutes never negative and rounding up; the blocked wording (no negative wait, "ask an administrator"); the DoS limiter being `None` when off or `0`; a ban comment that cannot break out of its quotes |
| `src/firewall.rs` unit | **+2 (29 → 31)** | an address is validated before it becomes a `ufw` argument (host names and shell fragments refused) and the `homevault-autoblock-<ip>` comment passes `validate_comment` while one with a quote does not — the two properties the argv depends on, asserted **without running ufw** |
| `tests/auto_block_integration.rs` | **14** | the admin matrix on all five routes (no token 401, member 403, admin 200); the shipped defaults on a fresh box; a policy round-trip that survives a reload; six impossible policies refused **with nothing stored**; a disabled policy allowed to keep impossible numbers (that is what keeps "off" reachable); **repeated failures banning the address**, which is then refused even with the **right** password (`403` + "temporarily blocked" + "administrator"), with `active`/`remaining_minutes` on the card; the ban **surviving a restart** (a second router/AppState over the same pool); **a ban never locking out another address** (the legitimate one signs in normally); loopback and non-addresses never banned while the admin can still sign in; nothing banned while the policy is off; a pardon lifting the block immediately (and an unknown address being a `400`, an empty one too); `clear` reporting three lifted rows; **an expired ban lifting itself through the login path** with the row swept, plus the explicit sweep endpoint's `{ removed }`; a ban surviving a **failed** firewall rule with `firewall_rule` following `ufw_available()`; the DoS row round-tripping and never storing above its ceiling |
| `tests/e2e/settings/autoblock.spec.ts` | **14 `test(`** | card shape (policy fields, save/refresh, list-or-empty); the defaults matching the API; the empty state naming what has to happen; the DoS field following its switch; a save round-trip that leaves `enabled` alone; an impossible policy refused with the server's reason as `role="alert"` and nothing stored; "off" always reachable; a blocked address rendered with its remaining minutes, reason and firewall flag, then **pardoned** (mocked — the list empties from the returned state); "lift every block" only when something is blocked; a failed pardon shown as `role="alert"` (never as success); a member refused on read/write/pardon; one label per field and an accessible name per switch; no duplicate ids; no horizontal overflow |

**No test invokes `ufw`.** The ban's firewall half is asserted as *generation*
(the address validation and the comment round-trip, in `src/firewall.rs`) plus
the recorded `firewall_rule` flag; the live effect (a rule appearing in
`ufw status`) needs root and stays a manual check.

**Design notes worth keeping** (each one a decision a later change could undo):

1. **The row is the authority, not the firewall.** A ban is stored even when the
   `ufw` rule fails, and `firewall_rule` records what was *actually* installed —
   a pardon and the expiry sweep trust that flag, so they can never delete a
   rule this feature did not add.
2. **Never the caller's own address, never loopback.** An admin who mistypes
   their password five times must not lock themselves out of the box they are
   standing in front of, and loopback is what the health checks and any reverse
   proxy use. `must_not_ban` is the single decision point.
3. **A ban is per address, never per account** — that is what answers "a blocked
   attacker cannot lock out a legitimate user from a different IP".
4. **Enforcement is scoped to authentication and to unauthenticated `/api`
   calls.** A session granted before the ban is not revoked by this layer
   ("revoke sessions" owns that), and an authenticated admin is never locked out
   by a middleware. Public document routes (`/share/*`) are outside it: a share
   link is its own credential.
5. **Expiry is self-healing on the read path.** The sweep runs on every
   enforcement check, so an expired ban lifts itself without a scheduler and
   without a restart; an expired-but-unswept row is still shown, labelled
   "block expired".
6. **Fail open, loudly.** A database error while reading bans logs an error and
   allows the request; a ban list that can lock the appliance is worse than the
   attack it prevents.
7. **The DoS row counts unauthenticated traffic only.** A family behind one
   router shares one budget; throttling the appliance's own dashboard polling
   before it stops an attacker would be a regression.

## 2026-10-31 — Connected clients and disconnect: QW-06

Written in this pass; **the Rust suites and the E2E spec were NOT executed**
(explicit instruction for this pass). Compilation was verified:
`cargo check --all-targets` clean, `npm run check` 0 errors / 0 warnings,
`npm run build` OK.

| Suite | Count | What it pins |
|---|---|---|
| `src/connected_clients.rs` unit | **16** | `smbstatus -j` sessions become one row each, with the share(s) from `tcons` and the open-file **count** from `open_files` (string and numeric pids both read; the `ipv4:` decoration stripped; a `hostname` equal to the address is not echoed as a host name); an SMB session with no share is not kickable ("nothing to close"); **no absolute path from `open_files` reaches the row** (asserted on the serialised payload); non-JSON `smbstatus` is an explicit error while `{}` is an empty list, not an error; `showmount -a` lines become never-kickable NFS rows and junk lines are skipped; `ss -tnp` keeps only `ESTAB` streams on the managed ports, handles IPv6 and a header row, marks a process-less stream visible-but-not-kickable, and folds several streams of one session into one row; the merge keeps SMB's account/share **and** the stream's pid, while leaving unrelated rows (including the same address on another protocol) separate; the two refusal messages (unknown id, NFS) |
| `tests/connected_clients_integration.rs` | **6** | the admin matrix on both routes (no token 401, member 403, admin 200); **the list always answers `200` with exactly three sources (`smb`, `nfs`, `sessions`), each carrying `available`/`count` and a non-empty `error` when unavailable**; the envelope (`clients`, `kickable`, `checked_at` as RFC 3339); no real path in the payload; an unknown client id is a `400` whose sentence says the client is not connected and to refresh; an empty id is a `400`; the NFS refusal explains the kernel and what to do instead |
| `tests/e2e/settings/connected-clients.spec.ts` | **9 `test(`** | the card with one status row per source; an unavailable source naming the tool's own reason; the list showing address/account/share/open-count with a Disconnect button on the SMB row and **none** on the NFS row (which shows "cannot be disconnected here"); disconnecting taking **two clicks** — nothing is sent on the first — then reporting the server's detail as `role="status"`; a refused disconnect as `role="alert"`; a member refused on read and kick; the disconnect button's accessible name quoting the client address; no duplicate ids; no horizontal overflow. The list and the kick are **mocked** (no real client is touched) |

**Nothing signals a real process and nothing calls `smbcontrol`.** The parsers
are unit-tested on representative `smbstatus -j` / `showmount -a` / `ss -tnp`
output, the endpoints on their contract, and the screen on its behaviour; the
live half (a real SMB session disappearing) is a manual check.

**Design notes worth keeping**:

1. **A failing source never fails the list.** Three independent probes, each
   bounded at 5 s and run on the blocking pool; an unavailable tool becomes a
   status row carrying its own error, so an empty section explains itself.
2. **No file paths.** `smbstatus` would hand us every open path; the row keeps
   the share name and a count. "Who is connected" is not "what are they
   reading".
3. **A kick re-collects before acting.** The request names a client id, never a
   pid; the pid used for `smbcontrol`/`kill` comes from the fresh collection.
   Without that, the endpoint would be a remote signal primitive.
4. **Only the kickable is offered.** SMB (`close-share`, keeping the session)
   and SFTP/FTP (a `TERM` to that connection's process) can be dropped; an NFS
   export cannot, and the row says so with the alternative.
5. **The two-click confirmation is text, not just colour.** The button changes
   its label and gains an outline, so the pending state survives a colour-blind
   read.

## 2026-10-31 — Per-share size quotas: QW-07

Written in this pass; **the Rust suites and the E2E spec were NOT executed**
(explicit instruction for this pass). Compilation was verified:
`cargo check --all-targets` clean, `npm run check` 0 errors / 0 warnings,
`npm run build` OK.

| Suite | Count | What it pins |
|---|---|---|
| `src/share_quota.rs` unit | **22** | the backend matrix — ZFS → `refquota`, a Btrfs **subvolume** → qgroup limit, a Btrfs **plain folder refused** with a reason that names "every share in it", XFS only with `prjquota`/`pquota` (and refused without, naming the mount option), four unsupported filesystems each naming itself *and* what does work, empty fstype → `unknown`; the XFS project id being stable across calls, normalised for a trailing slash, different per share, and never in the reserved low range; every argv (`zfs set refquota=<bytes>` / `refquota=none`, `zfs list -H -p -o used`, `btrfs qgroup limit <bytes|none>`, `btrfs qgroup show --raw`, `xfs_quota … project -s -p … <id>` then `limit -p bhard=<blocks>`, `report -p -b -N`) with XFS blocks rounded **up**; the parsers on real-shaped output including the "not found" answers that must stay `None` rather than guess; `findmnt -J` → the facts the selection needs (and its two error paths); **the decision** — fits, lands exactly on the ceiling, crosses it with all three numbers, `0` never full, an already-full share refusing a 1-byte file, and no wraparound at `u64::MAX`; **the acceptance criterion verbatim** (11 GB into a 10 GB share refused, both numbers in the sentence); the validation floor (1 MiB) and ceiling (1 PiB); `human_size` |
| `tests/share_quota_integration.rs` | **5** | the admin matrix on `GET`/`PUT`/`DELETE` (no token 401, member 403, admin past the guard); an unknown share **named** in the refusal and an unknown protocol refused as such; a quota under the floor / over the ceiling refused **with nothing stored**; the declared limit round-tripping in the database (per `(name, type)`, `0` deletes the row so "no quota" has one representation, `clear_all_for` dropping every protocol's row); and the state payload carrying every key the card needs, with an unsupported backend **always** explaining itself and claiming `quota_bytes: 0` |
| `tests/e2e/shares/quota.spec.ts` | **9 `test(`** | the quota action on every share card with an `aria-label`; the editor opening with a real `<label for>` and cancelling; a set ceiling rendering `used of quota`, the percentage and a `role="progressbar"` with `aria-valuenow`/`-max`/`-label`; an unsupported filesystem explaining itself (naming `ext4` and what does work) with **no** bar; saving sending **bytes** while the editor speaks GB, and the bar appearing from the answer; a refused save as `role="alert"`; a member refused on read and write; no duplicate ids with the editor open; no horizontal overflow with the bar and editor open. Every write is **mocked** |

**No test applies a real filesystem limit** — it needs ZFS/Btrfs/XFS as root and
would modify the host. The split is deliberate: argv + parsers + the decision are
unit-tested, the endpoint contract is integration-tested, and the screen is
E2E-tested with the endpoints intercepted.

**Design notes worth keeping**:

1. **Intent vs enforcement.** `share_quotas` records what the admin asked for;
   the filesystem holds the limit. The filesystem is changed **first**, and the
   row is only written when it accepted — so the card can never claim a ceiling
   that is not in force. Clearing does the same in reverse.
2. **Unsupported is a first-class answer.** A share on ext4/tmpfs reports
   `supported: false` with a reason naming what would work; the card hides the
   field rather than offering a limit that would not hold (same rule as QW-06's
   non-kickable NFS row).
3. **A Btrfs qgroup is per subvolume, not per folder.** Limiting a plain folder
   inside a subvolume would cap every share in it, so that case is refused with
   that exact reason instead of being silently widened.
4. **Usage is read back, never guessed** (`zfs list -p`, `btrfs qgroup show
   --raw`, `xfs_quota report`), which is what makes "10.0 GB of 10.0 GB" a fact
   rather than an estimate.
5. **Per-file, not per-chunk, re-check on upload.** Each checkpoint is a
   filesystem probe; a per-chunk probe would cost more than the upload. A file
   that crosses the ceiling is deleted, so the share is never left over its
   limit — the trade-off is documented rather than hidden.
6. **The refusal is `507` with the three numbers** (used, quota, incoming) and a
   pointer to the admin — the same sentence the pure `would_exceed` produces and
   the tests assert.
7. **Enforcement is complementary, not duplicated.** The Explorer refuses a
   transfer it can see coming; SMB/NFS clients are bound by the filesystem limit
   itself, which is why the feature is worth setting even if the web UI is never
   used.

## 2026-10-31 — Full configuration backup and restore: QW-11

Written in this pass; **the Rust suites and the E2E spec were NOT executed**
(explicit instruction for this pass). Compilation was verified:
`cargo check --all-targets` clean, `npm run check` 0 errors / 0 warnings,
`npm run build` OK.

| Suite | Count | What it pins |
|---|---|---|
| `src/config_backup.rs` unit | **13** | an unencrypted archive round-tripping with every entry byte for byte, every `sha256` a hex digest, the **skipped** file recorded with its reason; an encrypted archive round-tripping while its payload is **not readable in the file** (`[photos]` absent from the bytes) and its manifest still legible; four refusals — not-an-archive, `NewerFormat` with both versions and "Update HomeVault" in the sentence, `NewerSchema` with its own sentence, `PassphraseRequired`/`WrongPassphrase`/`NotEncrypted`; **secrets can never be exported in the clear** (the encoder itself refuses, and a blank passphrase is no passphrase); a tampered entry reported **by name**; a truncated archive reported as damaged; a non-JSON manifest reported as malformed (never a panic); the manifest's `entry`/`total_bytes` helpers; the **config allow-list contains no secret-looking path** (asserted against `tls`, `key`, `vpn`, `acme`, `ddns`, `syncthing`, `secret`) and the scrub list covers `sessions`, `trusted_devices`, `mcp_tokens`; a missing config file reported as skipped rather than failing the export |
| `tests/config_backup_integration.rs` | **6** | the admin matrix on state/export/restore (no token 401, member 403, admin 200 with `format_version`/`settings_schema`/`pending_restore: null`); a **full round-trip through the endpoints** — export → download → inspect (nothing staged) → restore (`requires_restart`, `READY` + `homevault.db` staged, `pending_restore` reported) → `apply_pending_restore` (marker consumed, `homevault.db.pre-restore` kept); secrets refused without a passphrase and with a short one, then an encrypted archive refusing no-passphrase and wrong-passphrase with the *clean* sentences and reading with the right one; **the acceptance criterion** — a default archive contains no live session token, no plaintext password, no MCP token row and no secret-bearing config, and its manifest says `includes_secrets: false`; a newer container refused with nothing staged; a non-archive refused; and three crafted archive ids (`../secrets`, `..%2Fsecrets`, `x/y`) refused |
| `tests/e2e/settings/config-backup.spec.ts` | **11 `test(`** | the card's export form, contents and restore input; "include credentials" revealing the passphrase field; a **pending restore announced** as `role="alert"` saying it applies at the next start; an export reporting the size and listing the archive with a download button; an impossible export (secrets without passphrase) as `role="alert"`; a **two-click** restore whose first click sends nothing and whose answer says "restart"; a refused archive (newer format) as `role="alert"`; a member refused on read and export; one `<label for>` per field and an accessible name per switch; no duplicate ids; no horizontal overflow with two archives listed. Every endpoint is **mocked** |

**Nothing is exported or restored by the tests** beyond the isolated
`DATA_DIRECTORY`: the E2E spec mocks both endpoints, the integration tests use a
temp data dir and a `sqlite::memory:` database, and no `/etc` file is written —
config files are *staged*, and the answer lists them in `needs_root`.

**Design notes worth keeping**:

1. **The container is self-describing and checksummed.** `HVCONFIG1` + a JSON
   manifest with a `sha256` per entry means a truncated or edited archive is
   refused **by name**, encrypted or not.
2. **Secrets are excluded by allow-list, not by denial-list.** The config paths
   a backup may carry are enumerated; the scrub list names the plaintext
   credential tables. Adding a new secret-bearing file therefore cannot leak by
   omission.
3. **The encoder enforces the encryption rule**, not just the handler.
4. **A missing config file is a recorded fact**, never a silent gap: a restore
   that lacks `smb.conf` says so.
5. **Restore is staged and applied at startup**, with `homevault.db.pre-restore`
   kept. A hot swap of a live SQLite pool is the one thing the feature must not
   pretend to do, and `READY` is written last so an interrupted staging is never
   applied.
6. **The passphrase travels in a header**, never in a URL, and the download is
   `no-store`.
7. **An archive id is untrusted input**: allow-listed characters *and* a
   canonicalised path confined to the archive directory.

## 2026-10-31 — Security Advisor (QW-10 / DSM-18) shipped

The last open quick win from `ROADMAP-DSM.md` (dashboard batch E) is implemented:
one read-only answer to "is this box safely configured?", above the Health
checks and for administrators only.

**Backend.** `src/security_advisor.rs` — 19 findings computed from an
`AdvisorFacts` value by a pure `evaluate()`, plus `collect()` which reads the
services the other screens already read (users + 2FA rows, TLS settings and
certificate metadata, ACME settings, `ufw` state and rules, the SMB config and
each share path's permission bits, share quotas, snapshot schedules, the login
auto-block, the **cached** update answer, the pool list and
`data/config-backups/`), every host probe inside one `spawn_blocking`.
`src/handlers/security_advisor.rs` — `GET /api/security/advisor`, admin-only,
no write route, nothing cached.

**Frontend.** `frontend/src/lib/components/health/SecurityAdvisorCard.svelte`,
rendered at the top of `Health.svelte` for `$isAdmin`; score as a real
`role="progressbar"`, findings worst-first with a deep link each, the rest behind
a native `<details>`.

**Tests.** 27 unit + 10 integration + 10 E2E (Firefox, live) is green; the whole
`health.spec.ts` re-run is 47/47. `src/i18n.rs` gained one parity test
(`security_advisor_keys_are_at_parity_in_every_locale`: every `health.advisor_*`
key exists and is non-empty in the four locales, no TOML `{name}` interpolation,
and the `%score%` / `%pass%` / `%count%` placeholders are the ones the card
passes). `cargo test --lib` is **2,012 passed / 0
failed**; `cd frontend && npm run check` is 0 errors / 0 warnings; the build is
clean and `cargo clippy --lib --tests` reports nothing in the new files.

**Deliberate deviations from the proposal.** (1) The findings are *English
sentences computed server-side*, not i18n keys: the report is also consumed by
API clients and the MCP surface, and a finding whose text depends on a browser
locale would be untranslatable there. The card's own chrome (title, severities,
summary, buttons) is fully translated in the four locales (`health.advisor_*`),
so the screen never shows a raw key. (2) There is no per-run history table: the
proposal mentioned storing the score for the dashboard, but a stored score can
disagree with the live reading, and the acceptance criterion ("fixing a finding
clears it on the next run without a restart") is what the live endpoint
guarantees — a dashboard tile can read the same endpoint later. (3) The update
check is read from the cache only, so opening the Health screen never contacts a
third party (the privacy rule the Settings version check already follows).

## 2026-10-31 — Global file search (QW-12) shipped

`GET /api/search/files` fans one query out over every root the caller's
Explorer sidebar lists, merges, deduplicates, ranks and caps the hits, and says
when the answer is partial. The ⌘K overlay runs it alongside its local index.

**Backend.** `src/global_search.rs` — the pure half: `validate_query` (2–128
**characters**, no `/`, `\`, NUL), `clamp_limit`, `dedupe_roots` (symlink-aware,
16-root cap), `merge_hits` (dedupe by path, exact → prefix → substring, shorter
names, directories first, deterministic path tie-break, truncation flag),
`tag_hits`, `hit_is_confined`. `src/handlers/search.rs` — the fan-out: per-root
1.5 s and total 4 s budgets, `timed_out` / `unreadable` reported per root. The
roots come from `explorer::explorer_roots_for_session`, the function
`handle_explorer_roots` now delegates to, so the sidebar and the search share one
definition — and an admin's search is bounded to those roots too.

**Frontend.** `stores/fileProvider.ts` (220 ms debounce, generation guard
against superseded answers, hit → row with the folder in the description),
`stores/explorerIntent.ts` (one-shot "open this folder, select this entry"),
`SearchBar.svelte` / `SearchResults.svelte` (the "looking…" line, the partial /
cut / unavailable notes), `Explorer.svelte` (`openExplorerIntent`).

**Tests.** 17 unit + 13 integration are **green** (`cargo test --lib`:
**2,030 passed / 0 failed**; `cargo test --test global_search_integration`:
13/13; `explorer_integration` 82/82 after the `explorer_roots_for_session`
extraction). 6 E2E written in `desktop.spec.ts` §6b and **not run** in this
session. `cd frontend && npm run check`: 0 errors / 0 warnings; the frontend was
rebuilt so the embedded assets match the sources.

**Deviations.** A `files_only` flag was added (the overlay needs file results
tight). The proposal's "bounded execution" became concrete numbers (16 roots,
1.5 s per root, 4 s total, 100 hits) and — the point — an **honest report**: the
answer carries `timed_out` / `unreadable` / `truncated` and the overlay renders
them, because a cross-root search that silently loses a share's results reads as
"your file does not exist". Content indexing stays out of scope, as proposed.

## 2026-10-31 — Phase 1 complete (P1.2 global file search, P1.3 passkeys)

Both remaining Phase 1 items ship in this change. Neither was exercised with a
test run in this session beyond the Rust suites below (the instruction was to
finish the phase without running the E2E).

**P1.2 — cross-root file search (QW-12).** `src/global_search.rs` (pure:
query validation, root dedupe, merge/rank, confinement), `handlers/search.rs`
(`GET /api/search/files`, fanned out over `explorer::explorer_roots_for_session`
— the sidebar's own root list), `stores/fileProvider.ts` + `stores/explorerIntent.ts`
+ the ⌘K overlay wiring. **17 unit + 13 integration green**; 6 E2E written
(`desktop.spec.ts` §6b).

**P1.3 — passkeys (WebAuthn).** `src/passkey.rs` (self-contained server-side
verifier: CBOR/COSE reader, `authenticatorData` parser, `ring` for ES256/EdDSA),
`handlers/passkey.rs` (two ceremonies + credential CRUD), migration `067`,
`storage/passkey_repo.rs`, `stores/passkey.ts`, the Account **Passkeys** card and
the Login screen's passkey button. **42 unit + 16 integration green** (the
integration suite signs with a real P-256 key), 13 E2E written (§18 Account,
§27 Login).

**Totals for this change.** `cargo test --lib`: **2,073 passed / 0 failed**
(2,030 before P1.3's 42 + 1 i18n parity test). `cargo test --test
global_search_integration`: 13/13. `cargo test --test passkey_integration`:
16/16. `cargo test --test explorer_integration`: 82/82 (after the
`explorer_roots_for_session` extraction). `cd frontend && npm run check`: 0
errors / 0 warnings; the frontend was rebuilt so the embedded assets match.

**Deviations, all documented in the guides.** The file search ships as **name
search** (no content index) with concrete budgets (16 roots, 1.5 s per root, 4 s
total, 100 hits) and a truthful partial-answer report. Passkeys verify
**nothing about the authenticator's provenance** (attestation is recorded as
evidence, not checked) and treat user verification as *preferred* rather than
required, so a hardware key without a PIN still works; the relying party is
derived from configuration (`HOMEVAULT_HOSTNAME` → ACME domain → host name) and
the Account card shows the origin, because a passkey bound to another address is
the one confusing failure mode.

## 2026-10-31 — Phase 2 complete (P2.1 Time Machine … P2.5 immutable snapshots)

All five Phase 2 items ship in this change. Every backend suite below was run
and is green, and the 43 Playwright tests were **written here and run on
2026-11-01** (see the verification-pass section at the end of this entry): a
headless browser has no macOS to back up, no rsync client, no second ZFS
machine, and `zfs hold`/`btrfs property` are privileged, so their mutating paths
are mocked — the specs prove the screens, not the daemons.

**P2.1 — Time Machine over SMB.** `src/shares.rs` renders one managed
`# homevault-smb-vfs` block per share (recycle + `vfs_fruit`, since
`vfs objects` may only be set once), `src/mdns.rs` publishes a per-share
`_adisk._tcp` service group with a stable `adVU`, and the SMB tab gains the
Time Machine card. **20 unit** in `src/mdns.rs` (advertisement shape, UUID
stability, group-per-share) plus the `shares.rs` VFS-block tests;
**`time_machine_integration` 8/8**.

**P2.2 — Rsync server.** `src/rsync_server.rs` (validation, rendering, apply),
`src/handlers/rsync.rs`, Shares → **Rsync** tab. **14 unit**, **`rsync_integration` 7/7** (admin-only on every mutation, a password written and never returned, path confinement against the share inventory).

**P2.3 — Backup destination.** `src/backup_target.rs`,
`src/backup_target_scheduler.rs`, `src/handlers/backup_target.rs`, the
**Backup destination** app, migration `070`. **14 unit** (12 + 2 scheduler),
**`backup_target_integration` 8/8`.

**P2.4 — Snapshot replication.** `src/replication.rs`,
`src/replication_scheduler.rs`, `src/handlers/replication.rs`, the
**Replication** app, migration `069`. **17 unit** (14 + 3 scheduler),
**`replication_integration` 7/7**.

**P2.5 — Immutable snapshots.** `src/snapshot_locks.rs`,
`src/snapshot_lock_scheduler.rs`, `src/handlers/snapshot_locks.rs`, migration
`068`, and the **Protection locks** section of Storage → Snapshots (plus the
per-snapshot protect action and the typed-confirmation release). **13 unit**
(11 + 2 scheduler) plus the prune refusal tests in `src/snapshot_scheduler.rs`,
**`snapshot_locks_integration` 7/7**.

**Verification run in this change.**

| Gate | Result |
|---|---|
| `cargo test` (all targets, 2026-10-31) | **3,374 passed / 0 failed** |
| `cargo test --lib` (2026-11-01, after the E2E fixes) | **2,150 passed / 0 failed** |
| `time_machine_integration` | 8/8 |
| `rsync_integration` | 7/7 |
| `replication_integration` | 7/7 |
| `backup_target_integration` | 8/8 |
| `snapshot_locks_integration` | 7/7 |
| `cd frontend && npm run check` | **0 errors / 0 warnings** |
| `cd frontend && npm run build` | green |
| Phase 2 Playwright, **Firefox** (2026-11-01) | **43/43 passed** |
| Phase 2 Playwright, **Chromium** (2026-11-01) | **43/43 passed** |

**Frontend defects this pass found and fixed** (the Phase 2 screens had never
been type-checked):

- `routes/Destinations.svelte` declared `let state = $state<TargetState | null>(null)`
  — the local binding shadowed the `$state` rune, so svelte-check reported
  "Block-scoped variable `$state` used before its declaration" plus 60 cascading
  errors. The binding is renamed `target`; the i18n placeholder key stays `state`.
- `routes/Destinations.svelte` and `routes/Replication.svelte` imported only
  `translate` from `stores/i18n` while their markup calls `$t(...)` (117 errors
  in total). Both now import `t` as well.
- **No `destinations.*` / `replication.*` translation existed in any locale.**
  Both screens shipped rendering raw keys. 49 + 40 keys added to
  `en/fr/de/es` (a 4-locale parity check would have caught it; the keys are now
  present in all four).
- **32 keys across the batch used TOML `{placeholder}` interpolation**, which the
  frontend translator cannot substitute (it replaces `%name%`). `wol_deleted`,
  `clients_source_count`, `autoblock_remaining`, `quota_bar_label`,
  `usage_uninstall_confirm`, `cap_reached`, `edit_widgets.*` and the config-backup
  keys all rendered the literal `{label}` in the shipped UI. Converted in all
  four locales; `ddns_custom_url_hint` (`{ip}`) and `snapshot_name_placeholder`
  (`auto-{date}`) keep their braces deliberately — there the braces are literal
  text the user types, not an interpolation.
- `routes/Replication.svelte` used `replication.keep_last` (an interpolation
  string) as a field label and `replication.enabled` as a counter label; split
  into `keep_last_label` and `footer_enabled`.

**New E2E coverage (verified 2026-11-01 — 43/43 on Firefox and Chromium).**

| Spec | Tests | What it locks down |
|---|---|---|
| `tests/e2e/shares/time-machine.spec.ts` | 6 | Only SMB shares get a row; the AFP/SMB copy; the optimistic toggle rolls back on refusal; size sent in GB with 0 = unlimited; 360px |
| `tests/e2e/shares/rsync.spec.ts` | 8 | The tab, the empty state, the picker offering only real shares, no request on load, a stored password never rendered, a refused save surfacing the reason, keyboard + 360px |
| `tests/e2e/backup/destinations.spec.ts` | 9 | Empty state, tenant row, over-quota wording, one-time secrets (and never in a URL or localStorage), refused creation, rotate-cancel, missing-binary warnings, failed load, 360px |
| `tests/e2e/backup/replication.spec.ts` | 11 | Empty state, capability line, job row, recorded error, client-side required fields, the exact payload a job sends, SSH fields, dry run copying nothing, run-now confirm, delete warning, icon-button names at 360px |
| `tests/e2e/storage/snapshot-locks.spec.ts` | 9 | The lock list, empty and lapsed wording, plain release of a lapsed lock, the typed-phrase gate and the phrase it sends, cancel firing nothing, a refused release surfacing the reason, days validation, 360px + no duplicate ids |

**Deviations, all stated in the guides.**

- **macOS acceptance is not automated.** Time Machine's generator is unit-tested
  against `vfs_fruit`'s documented recipe and the advertisement is unit-tested
  for its shape, but "a Mac backs up" needs a Mac; the guide says so instead of
  implying CI covers it.
- **Rsync over SSH is the documented path for anything leaving the LAN.** The
  daemon is plain `rsync://` (no TLS); the tab says so and points at the SFTP
  account.
- **Btrfs replication over SSH is refused.** `btrfs receive` to a remote path is
  not implemented; the API refuses the job instead of half-working.
- **Backup-destination quotas are enforced by revocation**, not by a filesystem
  quota: over quota the tenant leaves the htpasswd file. That is coarser than a
  hard ceiling (a run already in flight finishes) and it is what `rest-server`
  makes possible.
- **A tenant's repository password is recoverable by the appliance** (sealed, not
  hashed) because `rest-server` must read it; the token is hashed. The guide
  states the asymmetry.
- **No automatic full-appliance test restore.** Per-tenant `restic check` runs
  daily and replication records its last outcome; a full rebuild is the manual
  runbook in `docs/guides/disaster-recovery.md`.

### 2026-11-01 — Phase 2 E2E verification pass

The five Phase 2 specs were run for the first time against a live server
(rebuilt `target/debug/homevault`, scratch instance `PORT=8099`,
`DATA_DIRECTORY=/tmp/opencode/hv-p2-e2e`, admin `e2eadmin` seeded through
`POST /api/auth/setup`, isolated `HV_MDNS_HOSTNAME`). Command, per spec:

```
BASE_URL=http://localhost:8099 npx playwright test <spec> --project=firefox --reporter=list
```

| Spec | Firefox | Chromium |
|---|---|---|
| `tests/e2e/shares/time-machine.spec.ts` | 6/6 | 6/6 |
| `tests/e2e/shares/rsync.spec.ts` | 8/8 | 8/8 |
| `tests/e2e/backup/destinations.spec.ts` | 9/9 | 9/9 |
| `tests/e2e/backup/replication.spec.ts` | 11/11 | 11/11 |
| `tests/e2e/storage/snapshot-locks.spec.ts` | 9/9 | 9/9 |
| **Total** | **43/43** | **43/43** |

`cargo test --lib` was re-run after the fixes (**2,150 passed / 0 failed**);
`npm run check` stayed at 0 errors / 0 warnings and `npm run build` stayed green.

**One product defect, found by `snapshot-locks.spec.ts` 3.1.** `Storage.svelte`
→ `releaseLock()` set the refusal message and then called `loadSnapshotLocks()`,
whose first statement clears that same field — a refused release (pool busy,
hold owned by another process) closed the dialog and left the screen looking
exactly like a success. The reload now only runs on success, so the server's
reason stays on screen; the test asserts it.

**One robustness gap, found by `time-machine.spec.ts`.** The whole SMB tab
degraded to "This panel failed to render" when `GET /api/shares/smb/config`
answered without `allow_guest`: `loadSmbConfig` replaced the form state
wholesale, and the guest switch bound to `undefined`. The form now merges the
answer over `SMB_CONFIG_DEFAULTS`, so a partial payload costs one default, not
the screen. Verified with a probe that replays the old payload: the tab renders.

**One missing client-side guard, found by `rsync.spec.ts` 2.2.** The module
modal sent a mixed-case name and an empty account list straight to the server,
which refuses both. `saveRsyncModule()` now refuses them first with the reason
in the modal (`shares.rsync_name_lowercase`, `shares.rsync_need_account`,
`shares.rsync_need_share`, added to `en/fr/de/es`).

**Test defects fixed in the same pass** (all of them would have failed in CI):

- `time-machine.spec.ts` mocked a **stale SMB config shape**
  (`min_protocol`/`signing`/`guest_ok`/`available`/`running`) that the endpoint
  never returned; replaced by the real `SMB_CONFIG` fixture. The mock is what
  triggered the render failure above.
- All five specs: a freshly seeded server opens the **What's New modal**, whose
  bits-ui overlay swallows the first dock click. `beforeAll` now acknowledges the
  release through `POST /api/whats-new/seen` (the same fixture
  `desktop.spec.ts` and `docker.spec.ts` use).
- `destinations`/`replication`/`snapshot-locks`: `[role="dialog"]` also matched
  the **app window** (`WindowFrame` is `role="dialog"`), so every
  `.filter({ hasText })` was a strict-mode violation. Scoped to
  `[data-slot="dialog-content"]` (the theme Modal).
- `replication.spec.ts`: the glob `**/api/replication/**` does not match
  `GET /api/replication`, so three tests asserted against an empty real list;
  `**/api/replication**` matches both, and 2.2's duplicate route (which shadowed
  its own POST capture) is merged into one handler.
- `snapshot-locks.spec.ts`: the glob `**/api/snapshots/locks*` never matched
  `DELETE /api/snapshots/locks/7` (`*` does not cross `/`), so the release
  requests fell through to the real server; widened to `**`. The file also
  carried an unescaped apostrophe (`the server's reason`) that made the whole
  spec fail to parse — it had never been loaded.
- `rsync.spec.ts`: `1.1` asserted the literal `ssh://`, which the copy never
  contains (the template is `rsync -e 'ssh …'`); `2.2` used an unscoped
  `Save` button (three matched) and skipped itself when the button was disabled.

## 2026-11-01 — Phase 3 (P3.1 directory & SSO, P3.2 bandwidth control, P3.3 iSCSI)

### P3.1 stage 1 — Directory sign-in (LDAP / AD)

**Scope.** Sign in with the account a home or office already has on an LDAP or
Active Directory server, and turn directory groups into HomeVault roles. Remote
principals are **not** local accounts: `remote_identities` is the source of
truth and a `users` row is mirrored with the sentinel
`!directory-provisioned` so sessions and foreign keys keep working. Local
accounts keep their password, their 2FA and their ability to sign in when the
directory is down (break-glass). OIDC and SAML are the next two stages and reuse
the same table and the same role resolution.

**Backend.** `src/directory.rs` (model, validation, RFC 4515 escaping,
group→role resolution, bounded cache, `LdapBackend`), `src/handlers/directory.rs`
(5 endpoints + the login hook + the managed-password guard),
`src/storage/directory_repo.rs` (rows, mirror upsert, revocation), migration
`sqlx/migrations/072_directory.sql` (`users.auth_source`, `remote_identities`).
Settings live in `{DATA_DIRECTORY}/data/directory.json`; the service-account
password is sealed under the `directory-bind` scope.

| Test | What it pins |
|---|---|
| `directory::tests` (**24**) | `escape_filter_value` (every RFC 4515 metacharacter incl. NUL), `render_filter` replacing *every* occurrence, `valid_attribute` / `valid_dn` / `valid_filter_template` / `valid_server_url` (each rejects its adversarial shapes), `normalize_role`, `validate` for every refusal and for a half-filled-but-disabled document, `resolve_role` (highest wins, full DN vs bare `cn`, case-insensitivity, default role), `GroupRole` round-trip, `cache_key` isolation, the cache (TTL expiry, entry cap, `0` disables), `LdapBackend::attributes` case-insensitivity, and the sign-in orchestration against a **fake backend** — a cached search result still verifies the password, a wrong password drops the entry, and every outage maps to `Unavailable` rather than `InvalidCredentials` |
| `directory_integration` (**15/15**) | auth matrix (all five routes 401/403 for a member, 401 with no session), the full payload (`settings`/`status`/`identities`/`counts`/`break_glass`), a half-filled enabled config reporting a `problem` while `usable` stays false, the bind password **absent from the response and from `directory.json`** (sealed), eleven contradicting bodies each refused with a usable sentence, a mapping edit demoting existing principals immediately, a directory principal refused a local account's name, disable and forget both revoking sessions, an unknown principal as a 404, a directory sign-in refused while the integration is off, an **unreachable** directory (`ldap://127.0.0.1:1`) answering 503 **without** a lockout entry or an IP ban, the test panel reporting a failure without persisting anything, an inline invalid test refused before any connection, and a directory password refused by the change-password endpoint with a message naming the directory |

No integration test needs a live LDAP server: the unreachable-URL case is the
fail-safe path, and everything else is either validated before a connection or
covered by the fake backend in the unit tests.

**Frontend.** `frontend/src/routes/Directory.svelte` — `PageLayout` +
`SectionCard`, a `role="status"` banner that always states whether directory
sign-in is on, what is broken, and how many local administrators remain, a
write-only service password with `Forget`/`Keep`, a test panel that tries the
form as typed, group→role rows with per-row `aria-label`s, and the principal
list. Desktop registry: `appDefs` id `directory`, System category, admin-only,
gradient token `--hv-app-directory-grad`; `[directory]` (100 keys) +
`app_directory_*` in `en/fr/de/es`.

**E2E.** `tests/e2e/directory/directory.spec.ts` — 23 tests, **run green on
2026-11-01: 23/23 Firefox and 23/23 Chromium** against a scratch instance
(`PORT=8080`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded through
`POST /api/auth/setup`). Every `/api/directory*` call is intercepted with
`page.route`, so the spec proves the screen and never needs a directory server.

| Section | Tests |
|---|---|
| 1 — Load and health | the app opens from the launcher with the saved server and base DN and an empty password field; `role="status"` reports sign-in on plus the break-glass count; off says local accounts are unchanged; a `problem` shows the server's sentence; zero local admins is called out |
| 2 — Saving | the whole document is sent and `bind_password` is `null` when untouched; a typed password replaces the sealed one and `""` forgets it; a refusal keeps the typed value on screen and shows the server sentence; the directory type swaps only untouched defaults; the attribute names and cache stay behind **Advanced**; an empty mapping row is dropped; mapping rows are added and removed |
| 3 — Testing | the service-account test posts the form as typed and saves nothing; a sign-in test posts the credentials and reports the role it would grant; a failure shows its reason; the sign-in test needs a name |
| 4 — Accounts | disabling flips the badge, the counts and the action in one pass; forgetting asks first and cancelling sends nothing; confirming removes the row; an empty list reads as an instruction |
| 5 — Accessibility | the enable switch is named by its visible label exactly once (`aria-labelledby`, no duplicate inline label); icon-only row actions carry the account name; every visible input resolves a label |

**Two accessibility defects the suite found** (both in shared components, both
fixed rather than worked around): `theme/Toggle` was being given the visible
label *and* an `aria-labelledby` pointing at it, so the switch was announced
twice — the screen now uses `aria-labelledby` alone; and `theme/Button` accepted
`aria-label` through `...rest` but did not declare it, so `svelte-check` rejected
the only accessible name options for an ambiguous row button. The prop is now
declared and documented, which unblocks every screen that needs an icon-only or
disambiguated button name.

**Verification gates.**

| Gate | Result |
|---|---|
| `cargo test --lib directory` | 24/24 |
| `cargo test --test directory_integration` | 15/15 |
| `cd frontend && npm run check` | 0 errors / 0 warnings |
| `cd frontend && npm run build` | green |
| `directory.spec.ts` Firefox / Chromium | **23/23** / **23/23** |

**Docs.** `docs/guides/directory-sso.md`, an `INDEX.md` row, a `FeatureDoc`
(`directory-sso` ← `directory-sso.md`, the 62nd feature guide,
`docs/guides/rag.md` count 61 → 62) and `SCREENS.md` §34 (app table row 31,
System category, totals 35 → 36 screens, `appDefs` 28 → 29).

### P3.1 stage 2 — Single sign-on with OpenID Connect

**Scope.** The second provider on the same screen, the same `remote_identities`
table, the same group→role resolution and the same break-glass rule. An admin
registers an application at an identity provider, pastes the issuer, client ID,
optional secret and explicit callback URL, and from then on the login card
offers a button that leaves for the provider and comes back with a session.

**Zero new dependencies.** `src/oidc.rs` (discovery, authorization-code + PKCE,
token exchange, JWKS, ID-token verification, settings persistence and sealing)
and `src/handlers/oidc.rs` (seven routes) are built on crates the appliance
already ships: `ring` 0.17 for signing/verification, `base64`, `url`, and
`reqwest` for the HTTP calls. Settings live in
`{DATA_DIRECTORY}/data/oidc.json`; the client secret is sealed under the
`oidc-client-secret` scope. No migration: `remote_identities` already carries
`provider`, and migration `072` already added `users.auth_source`.

| Test | What it pins |
|---|---|
| `oidc::tests` (**36**) | `sanitize_username` (an `@domain` suffix dropped, illegal characters collapsed to `_`, a leading digit prefixed, the 64-char truncation, reserved names refused as a claim), issuer/redirect validation (`https` required, loopback may be `http`, must end in `/api/auth/oidc/callback`, no fragment/query), scope validation (`openid` required), `validate` for every refusal, `resolve_groups_role` reuse, the `ExpiringStore` (TTL expiry, entry cap, single use), `verify_id_token` against a real `ring` ES256 signer (accepts a good token; refuses `alg: none`, an HMAC signature, a wrong `kid`, a bad signature, a wrong `iss`/`aud`/`nonce`, an expired token and a key type that does not match the `alg`), JWKS parsing and the refetch-only-on-unknown-`kid` rule, and the sealing round-trip of the client secret |
| `oidc_integration` (**25/25**) | an in-process **fake IdP** signing with a real ES256 `ring` key: discovery + JWKS + token endpoint, so the whole PKCE flow runs end to end against the assembled router — `start` sets `hv_oidc_state` and answers an authorization URL carrying the state, the callback exchanges the code, verifies the token, mirrors the principal and answers **`303`** to `/login#sso=…`, and `/complete` returns the `LoginResponse`; then the auth matrix (settings routes 401/403, the sign-in routes public), sealing (the secret is absent from the response and from `oidc.json`), role resync when a mapping changes, and the refusals: missing cookie, replayed state, expired state, provider error, integration off mid-flow, unreachable provider, `alg: none`, an impostor key, a tampered token, a claim with no usable name, a reserved name, a collision with a local account, a disabled principal, a spent or expired ticket, and break-glass left intact |

The fake IdP is the reusable harness (`start_idp`, `run_sign_in`, `signer()`):
no hosted provider, no network, and the assertions are deterministic.

**Frontend.** `frontend/src/routes/Directory.svelte` gained a two-button
segmented control (`role="group"` + `aria-pressed`, deliberately **not** an
ARIA tablist — the arrow-key pattern would have been half-implemented) that
switches the LDAP form and the OIDC form; the toolbar Save and the footer counts
follow the active tab, and the remote-account list is shared with a provider
badge. `Login.svelte` gained the single sign-on button
(`data-testid="login-sso"`), `consumeSsoReturn()` for the `#sso=` ticket, and a
`t()` that now asks the server translation map first and falls back to the
bundled seed — the seed never carried the newer `login.*` keys, which is why
passkey and single sign-on labels previously rendered as raw keys.
`authStore.completeSso` owns the token-before-user ordering and the non-admin
invalidation, exactly like `login()`. `[oidc]` is 50 keys in `en/fr/de/es`.

**E2E.** `tests/e2e/directory/directory.spec.ts` grew to **35 tests**, run green
on 2026-11-01: **35/35 Firefox and 35/35 Chromium** against a scratch instance.
`mockApi` gained `**/api/oidc**` interception with `oidcSettings`/`oidcTests`/
`oidcRefuse` recorders, plus `oidcSettings()` and `carol()` fixtures.

| Section | Tests |
|---|---|
| 6 — Single sign-on settings | the segment switches forms and Save follows the active one (and never writes the other half); the client secret is write-only — stored, replaced, then cleared with `""`; the check reads the typed configuration and saves nothing; a refused save keeps what was typed and shows the server sentence; one account list with the provider named and tab-scoped counts; the switch is a named group of pressed buttons, not a half tablist |
| 7 — Signing in with single sign-on | the button appears only when the appliance offers it, with the configured text and a password form still present; clicking it asks the server where to go and then really leaves; a refused sign-in lands on the login card as a sentence and the code is consumed once; the one-shot ticket is exchanged and the desktop opens; a valid but non-admin session is refused *and* logged out server-side; a malformed ticket is refused with no network call |

**Two defects the run found** (both in `Login.svelte`, both fixed): the SSO
failure was being routed to the error modal, which is a child component that
initialises closed and swallowed an open in the same tick as its first mount —
a return from the provider is exactly that case — so SSO failures now render in
the inline alert (`login-inline-error`); and `t()` only read the bundled seed
dictionary, which lacks every newer `login.*` key, so the button read
`login.sso_generic` until the server map was consulted first.

**Verification gates.**

| Gate | Result |
|---|---|
| `cargo test --lib oidc` | 36/36 |
| `cargo test --test oidc_integration` | 25/25 |
| `cargo check --all-targets` | clean |
| `cd frontend && npm run check` | 0 errors / 0 warnings |
| `cd frontend && npm run build` | green |
| `directory.spec.ts` Firefox / Chromium | **35/35** / **35/35** |

**Docs.** `docs/guides/directory-sso.md` extended to both providers (endpoints
table, the full OpenID Connect section, the refusal table, the new failure rows,
the stages list), the `INDEX.md` row reworded, the `FeatureDoc`
(`directory-sso`) rewritten to cover both halves — the count stays **62** —
and `SCREENS.md` §34 now documents the segment, the single sign-on panel and
the OIDC invariants.

**Full-run counts after the stage** (2026-11-02): `cargo test` green — lib
**2241 passed / 0 failed**, **81** integration suites, **3519** tests in total,
11 ignored (environment-conditional), 0 failed; `cargo fmt` and
`cargo clippy` clean on the Phase 3 modules (`oidc`, `directory`, `iscsi`,
`traffic_control` and their handlers/repos).

### P3.1 stage 3 — SAML 2.0

**Scope.** The third tab of the same screen: HomeVault as a SAML 2.0 service
provider. `src/saml.rs` holds the model, validation, the XML tree, exclusive
canonicalisation, signature and assertion verification, the AuthnRequest and the
service-provider metadata; `src/handlers/saml.rs` holds the seven routes;
settings live in `{DATA_DIRECTORY}/data/saml.json`. No new crate: `xml` parses,
`ring` verifies and digests, `x509-parser` reads the IdP certificate, `flate2`
inflates the redirect binding. No migration — `remote_identities.provider`
already carries `saml`.

| Test | What it pins |
|---|---|
| `saml::tests` (**28**) | the XML tree and exclusive C14N (attribute ordering by namespace then local name, an unused namespace declaration dropped, a used one kept, text escaping), the DTD refusal (XXE and entity expansion), `same_url` (trailing slash, scheme, host), the request-ID store (TTL, cap, single use), `sanitize_username` and the reserved-name list, settings validation (`https` only, entity ID/SSO URL required, an empty certificate list refused, the label cap), `resolve_groups_role` reuse, and `verify_assertion` against a real `ring` RSA signer: a good assertion accepted; a tampered body, a wrong certificate, an unsigned assertion, a wrong audience, a wrong destination, an expired `NotOnOrAfter`, a replayed request ID and a missing login-name attribute each refused with their own reason |
| `saml_integration` (**16/16**) | the assembled router: the auth matrix (settings routes 401/403, the three sign-in routes public), `GET /api/saml` shape, a save round-trip, the metadata document, `POST /saml/test` reading a fake IdP's metadata and saving nothing, the full sign-in (start → ACS → `303` to `/login#sso=…` → `/complete` returns the `LoginResponse`), the mirrored principal and its role, a mapping change demoting immediately, and the refusals: missing cookie, replayed request ID, integration off mid-flow, bad signature, expired assertion, a reserved name, a collision with a local account, a disabled principal, a spent ticket, and break-glass left intact |

**Frontend.** `Directory.svelte` gained the third segment button, the SAML panel
(status card, an on/off switch, the metadata import card, the provider card, the
attribute and group→role editors, the copyable service-provider metadata), the
`SAML` provider badge in the shared account list, and a tab-aware toolbar Save
and footer counts. `[saml]` is 49 keys in `en/fr/de/es`.

**E2E.** `tests/e2e/directory/directory.spec.ts` grew to **39 tests**.
`mockApi` gained `**/api/saml**` interception with `samlSettings`/`samlTests`/
`samlRefuse` recorders, plus `samlSettings()` and `dave()` fixtures.

| Section | Tests |
|---|---|
| 6 — Single sign-on settings | 6.5 now also checks the SAML tab's badge, description and footer counts; 6.6 counts three segment buttons; 6.7 the SAML tab saves only the SAML half and can be turned on; 6.8 reading the provider metadata fills the form and saves nothing; 6.9 a refused SAML save keeps the typed values and shows the server sentence |
| 7 — Signing in with single sign-on | 7.1–7.6 unchanged for OpenID Connect; 7.7 adds the second button — two providers render two buttons with their own text, and clicking the SAML one starts the SAML flow only |

**Two defects the run found** (both fixed): the SAML panel had **no on/off
switch**, so the status card told the admin to turn it on with no way to do it —
the enable card was added; and the two mutating settings routes parsed the body
before checking the admin session, so an anonymous `PUT` answered `415` instead
of `401` — both now authenticate first and parse afterwards.

**Verification gates.**

| Gate | Result |
|---|---|
| `cargo test --lib saml` | 28/28 |
| `cargo test --test saml_integration` | 16/16 |
| `cargo fmt --all -- --check` | clean |
| `cd frontend && npm run check` | 0 errors / 0 warnings |
| `cd frontend && npm run build` | green |
| `directory.spec.ts` | 39 tests, **39/39 green on Firefox and Chromium** (scratch server, 2026-11-02) |

**Docs.** `docs/guides/directory-sso.md` extended to three providers (endpoints
table, the full SAML section with its refusal table, the new failure rows, the
security notes, the stages list), `docs/API.md` gained the eight SAML routes,
the `INDEX.md` row reworded, and the `FeatureDoc` (`directory-sso`) rewritten to
cover the third half — the count stays **62**.

### P3.2 — Traffic control

**Scope.** Per-service egress shaping behind one HTB queue: `smb`, `nfs`, `ftp`,
`webdav`, `sftp` each get a rate in kbit/s (`0` = unlimited), the policy carries
an optional weekly schedule and a pause-all flag, and management ports are never
shaped.

**Backend.** `src/traffic_control.rs` (policy model, validation, schedule
arithmetic, `tc` argv generation, apply/teardown, JSON persistence, scheduler),
`src/handlers/traffic_control.rs` (HTTP surface). The policy is one JSON
document at `{DATA_DIRECTORY}/data/traffic_control.json` — no migration.

| Test | What it pins |
|---|---|
| `traffic_control::tests` (**15**) | `valid_interface_name` (length, charset, `lo`), `validate` (unknown/duplicate service, rate bounds, window days and times, enabled-requires-interface), `ScheduleWindow::covers` incl. the midnight and Sunday→Monday wrap, the exact `build_commands` pipeline, `teardown_command`, `capabilities` (every service once, `supports_download_limit: false`), `inactive_reason_code` for each gate, `load`/`save` round-trip with an absent file |
| `traffic_control_integration` (**6/6**) | auth matrix (reads need a session, `PUT`/`pause`/`apply` need an admin → 401/403), the `GET` payload shape (`settings`/`capabilities`/`interfaces`/`state`, `management_ports == [22,80,443,8443]`), seven validation refusals, `pause` rejecting a non-boolean, a policy round-trip through `PUT` → fresh `GET` → the JSON file on disk, pause ⇄ resume |

No integration test names a real interface: `apply_sync` returns
`"No interface is selected"` before it can run `sudo tc`, so the suite never
deletes a root qdisc on the host that runs it. The command pipeline is covered
by the unit tests instead.

**Frontend.** `frontend/src/routes/TrafficControl.svelte` + desktop registry
(`appDefs` id `traffic-control`, admin-only, Network category, gradient token
`--hv-app-traffic-control-grad`), `[traffic]` + `app_traffic-control_*` keys in
`en/fr/de/es`.

**E2E.** `tests/e2e/network/traffic-control.spec.ts` — 11 tests, **run green on
2026-11-01: 11/11 Firefox and 11/11 Chromium** against a scratch instance
(`PORT=8096`, isolated `DATA_DIRECTORY`, `e2eadmin` seeded through
`POST /api/auth/setup`). Every `/api/network/traffic-control*` call is
intercepted with `page.route`, so the spec proves the screen and never touches
`tc`.

| Section | Tests |
|---|---|
| 1 — Load and status | the app opens from the launcher with every service row and rate input; `role="status"` reports shaping on; a stopped policy shows its reason |
| 2 — Editing | a rate edit is sent as kbit/s for all five services; a refused save surfaces the server's own reason |
| 3 — Schedule | a window is added and a day toggled (default Mon–Fri, Saturday off → on) and posted; a window is removed with no `PUT` |
| 4 — Pause and apply | pause-all posts `{paused:true}`, updates the status and flips the button to Resume; apply-now re-applies without saving |
| 5 — Accessibility | each weekday toggle measures ≥44 px in both axes and carries `aria-pressed`; every rate input resolves a non-empty label |

**Two spec defects found and fixed by the run** (both were mine, not the
product's): the dock tile is now located by
`.lp-card[data-app-id="traffic-control"]` (the previous `hasText` selector
matched the launchpad card description too), and the What's New overlay is
acknowledged in `beforeAll` + dismissed after navigation — on a freshly seeded
server its overlay swallows the first dock click. A third assertion was simply
wrong about the fixture: a new schedule window defaults to Monday–Friday, so
Wednesday is *enabled*; the disabled day asserted is Saturday.

**Verification gates.**

| Gate | Result |
|---|---|
| `cargo test --lib traffic_control` | 15/15 |
| `cargo test --test traffic_control_integration` | 6/6 |
| `cd frontend && npm run check` | 0 errors / 0 warnings |
| `cd frontend && npm run build` | green |
| `traffic-control.spec.ts` Firefox / Chromium | **11/11** / **11/11** |

**Docs.** `docs/guides/traffic-control.md`, an `INDEX.md` row, a `FeatureDoc`
(`traffic-control` ← `traffic-control.md`, the 60th feature guide) and
`SCREENS.md` §32 (app table row 29, Network category, totals 33 → 34 screens,
`appDefs` 26 → 27).

### P3.3 — iSCSI exports

**Scope.** Publish a disk over iSCSI through the kernel's LIO stack: file and
block backstores, targets (IQNs), portals, LUNs, initiator ACLs and optional
per-initiator CHAP. The feature is opt-in twice — an admin switch **and** a host
that provides `targetcli` plus the LIO modules — and saving metadata never
touches the host: only `POST /apply` and turning the feature off run commands.

**Backend.** `src/iscsi.rs` (model, validation, capability probe, `targetcli`
argv generation, apply/teardown, backing-file lifecycle), `src/handlers/iscsi.rs`
(HTTP surface), `src/storage/iscsi_repo.rs` (rows), migration
`sqlx/migrations/071_iscsi.sql`. The switch and the last apply time live in
`{DATA_DIRECTORY}/data/iscsi.json` (atomic JSON — no migration for a
single document).

| Test | What it pins |
|---|---|
| `iscsi::tests` (**16**) | `valid_backstore_name`, `valid_iqn` (prefix, reverse-domain part, charset, length, `..`), `valid_portal_address` (IP literals only), `valid_portal_port`, `valid_lun`, `validate_chap` (both halves together, 12–16 characters per RFC 7143, printable ASCII), `validate_target` over every collection (no portal, duplicate portal, duplicate LUN, duplicate initiator case-insensitively), `chap_scope` per target+initiator and case-insensitive, the fileio-vs-block backstore commands, `backstore_delete_command` naming the object and never the file, the exact target pipeline **with `enable` last**, teardown naming exactly one IQN, `module_commands` covering only the missing modules, `redact` masking `password=` and leaving other `key=value` alone, settings defaulting to off, `delete_backing_file` refusing a relative path |
| `iscsi_integration` (**8/8**) | auth matrix (reads need a session, all eight mutations need an admin → 401/403), the `GET` payload shape (`settings`/`capabilities`/`backstores`/`targets`/`defaults`/`paths`/`state`), nine validation refusals (space in a name, path outside the data directory, block path outside `/dev`, size under 1 MiB, unknown kind, no portal, bad IQN, LUN on a missing disk, duplicate LUN, duplicate initiator, CHAP without a secret, short secret) with nothing persisted, the disk→export round trip (LUN `backstore_name`, `has_secret`, `lun_count`, the secret **never** echoed), an update with `secret: null` keeping the sealed blob, an IQN rename refused as "cannot be changed in place", deleting a disk a LUN still serves refused, `delete_file=true` needing `confirm=<name>`, a block store refused when the device does not exist, and apply reporting `no_targetcli` with an **empty** command list on this host |

Nothing in the suite touches the machine: this host has no `targetcli`, so
`apply_sync` returns `reason_code: "no_targetcli"` with zero commands before it
can run anything. The `else` branch asserts the report is merely coherent, so
the test also passes on a machine that does have the LIO stack.

**Frontend.** `frontend/src/routes/Iscsi.svelte` — `PageLayout` + `SectionCard`,
a `role="status"` banner, `theme/Modal` for both editors (focus trapped and
restored), a focus-trapped `ConfirmDialog` for the export delete, shared `.hv-ic`
44px hit squares on every row action, an explicit enable/disable **Button** (not
a `Toggle`) with the consequence named. Desktop registry: `appDefs` id `iscsi`,
Storage category, admin-only, gradient token `--hv-app-iscsi-grad`; `[iscsi]` +
`app_iscsi_*` keys in `en/fr/de/es`.

**E2E.** `tests/e2e/storage/iscsi.spec.ts` — 16 tests, **run green on 2026-11-01:
16/16 Firefox and 16/16 Chromium** against a scratch instance (`PORT=8080`,
isolated `DATA_DIRECTORY`, `e2eadmin` seeded through `POST /api/auth/setup`).
Every `/api/storage/iscsi*` call is intercepted with `page.route`, so the spec
proves the screen and never runs `targetcli`.

| Section | Tests |
|---|---|
| 1 — Load and status | the app opens from the launcher with both disks and the export; `role="status"` reports "1 export(s) reachable"; saved-but-not-applied says so; a host without the LIO stack shows all three missing pieces and **removes** the editors |
| 2 — Disks | adding a disk sends the size in bytes (250 GiB), the kind and the read-only flag; a refused save keeps the form open with the server's own reason |
| 3 — Exports | a new export prefills the IQN and portal from the server defaults and sends portals, LUNs and CHAP; no initiator sends `acls: []`; the LUN editor says a disk is needed first and disables Add |
| 4 — Deleting | a disk a LUN still serves is refused in a toast **without** sending a request; deleting the backing file needs the checkbox **and** the typed name (`delete_file=true&confirm=spare` reaches the server) |
| 5 — Apply | Apply now clears `pending` and the banner flips to reachable; the feature switch saves `{enabled:false}` immediately |
| 6 — Accessibility | row actions carry `aria-label` naming their object; every editor input resolves a label; the editor traps focus and returns it to the trigger on Escape |

**One real defect the suite found:** the export editor did not fit a 720px
viewport, so the Save button was unreachable and Playwright refused to click it.
`frontend/src/lib/components/ui/dialog/dialog-content.svelte` (the shared
shadcn dialog behind `theme/Modal`) now caps at
`max-h-[calc(100dvh-2rem)] overflow-y-auto overscroll-contain`. That fixes every
tall modal in the app, not just this one. A second finding was in the test
itself: `getByRole('checkbox').check()` is unreliable on bits-ui checkboxes —
`.click()` is what a person does, and it is what the spec now does.

**Verification gates.**

| Gate | Result |
|---|---|
| `cargo test --lib iscsi` | 16/16 |
| `cargo test --test iscsi_integration` | 8/8 |
| `cargo test --lib dependencies` | 21/21 (after `targetcli-fb` was added to `installer/install.sh`) |
| `cd frontend && npm run check` | 0 errors / 0 warnings |
| `cd frontend && npm run build` | green |
| `iscsi.spec.ts` Firefox / Chromium | **16/16** / **16/16** |

The dependency test caught a real omission: `DependencySpec` claimed
`targetcli-fb` was installed by `installer/install.sh` while the script never
mentioned it. It is now an `install_optional` line in the storage block, so a
machine without the package still installs and the app reports the gap.

**Docs.** `docs/guides/iscsi.md`, an `INDEX.md` row, a `FeatureDoc` (`iscsi` ←
`iscsi.md`, the 61st feature guide, `docs/guides/rag.md` count 60 → 61) and
`SCREENS.md` §33 (app table row 30, Storage category, totals 34 → 35 screens,
`appDefs` 27 → 28).

---

## 2026-11-05 — SMB/NFS config contract, multichannel

Closes tasklist P0-1 → P0-4 and P1-1 (`HOMEVAULT_TASKLIST_A_FAIRE.md`).

**Behaviour.** `NfsConfig` (`networks`, `default_access`, `version`,
`root_squash`, `custom_options`) is now validated and rendered into
`/etc/exports` (one client clause per network, `vers=`, squash, per-share
overrides by class); saving the service form rewrites every managed line and
runs `exportfs -ra`. `version` defaults to `auto` (kernel negotiation).
SMB refuses `domain`/`server`/`ads` (no winbind) and NT1/SMB1, writes both
`server min protocol` and `server max protocol = SMB3`, applies allowlisted
`additional_params`, and exposes `multichannel`.

**New unit tests (`src/shares.rs`, +11):**
`test_nfs_export_line_applies_networks_and_share_overrides`,
`test_nfs_version_auto_omits_vers`,
`test_validate_nfs_config_rejects_injection_and_bad_values`,
`test_validate_smb_config_rejects_unimplemented_domain_auth`,
`test_smb_extra_params_are_allowlisted`,
`test_smb_global_params_renders_every_field`,
`test_smb_config_deserializes_without_multichannel`,
`test_get_smb_config_normalizes_legacy_values`,
`test_smb_line_key_ignores_comments_and_sections`,
`test_ftp_config_text_renders_every_field`,
`test_ftp_user_list_and_validation`.
Two existing tests were updated for the new defaults
(`test_nfs_export_line_defaults_and_round_trip`,
`test_nfs_disabled_tag_round_trips_as_a_disabled_share`), and
`test_nfs_config_default` now expects `version = "auto"`.

**New E2E (`tests/e2e/shares/shares.spec.ts`, +3):** 4.9 (SMB advanced offers
only User/Share, SMB2/SMB3, multichannel, and the PUT body carries
`multichannel: false`), 4.10 (FTP advanced sends the account whitelist as a
list) and 4.11 (NFS advanced offers Auto/3/4/4.2 and the PUT body carries
`version: "auto"` + `networks: ["*"]`).

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib` and `npm run check` were run and are clean
(0 errors; only pre-existing warnings). Run before release:
`cargo test --lib shares`, `cargo test --test shares_integration`, and
`npx playwright test tests/e2e/shares/shares.spec.ts --project=firefox -g "4.9|4.10|4.11"`.

---

## 2026-11-05 — password expiry and metrics retention

Closes tasklist **P1-2** and **P1-3**.

**Password expiry (P1-2).** Migration `079_password_policy.sql` adds the
singleton `password_policy` row (`expiry_days`, default `0` = never) and
`users.password_changed_at` (backfilled from `created_at`). Both password
write paths touch the marker; `AuthService::enforce_password_expiry` runs after
a successful credential check and sets `must_change_password`, which the
existing guard already limits to the change flow. Admin endpoints
`GET/PUT /api/settings/password-policy` (0–3650, `400` outside), Settings →
Security card, i18n ×4.

- Unit (`src/storage/password_policy_repo.rs`, 4): `0` never expires, exact
  boundary is not expired / one second later is, future timestamp, RFC 3339
  parse.
- Integration (`tests/password_policy_integration.rs`, 3): admin matrix +
  bounds, expired password flagged then clock restarted by a change, `0` never
  expires and a fresh password is not flagged.
- E2E (`tests/e2e/settings/settings.spec.ts`, 12.7): the card saves
  `expiry_days` through an intercepted PUT.

**Metrics retention (P1-3).** Migration `080_metrics_retention.sql` adds
`metrics_settings.retention_days` (default 8). The collector reads it on every
tick (no restart), `retention_secs` clamps to 1–365, and the Metrics header
offers 8 / 30 / 90 days.

- Unit: `retention_seconds_are_clamped`.
- Integration (`tests/metrics_integration.rs`): read = any session, write =
  admin, round-trip, `0`/`366` refused without changing the stored value.
- E2E (`tests/e2e/metrics/metrics.spec.ts`, 2.2): the select issues the PUT.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo test --lib --no-run`, `cargo test --test
password_policy_integration --no-run`, `cargo test --test metrics_integration
--no-run` and `npm run check` are clean. Run before release:
`cargo test --lib password_policy`, `cargo test --test password_policy_integration`,
`cargo test --test metrics_integration`, then E2E settings 12.7 and metrics 2.2.

---

## 2026-11-05 — generated OpenAPI 3 contract

Closes tasklist **P1-4**.

**Behaviour.** `GET /api/openapi.json` (`require_admin`) serves an OpenAPI
3.0.3 document built by `src/openapi.rs` from the Axum route table itself:
`src/web/mod.rs` plus the `/api/network`, `/api/vpn`, `/api/rescue`,
`/api/terminal` and `/api/vm` sub-routers. The document declares the
`X-Session-Token` / `X-CSRF-Token` security schemes, the optional
`X-HomeVault-Contract` header, the `400/401/403/426/500/501/503` responses,
`security: []` on authentication flows, and `x-homevault-auth` per operation.
WebSocket, SSE, one-shot-ticket and binary upload/download transports are
excluded from `paths` and named in `x-homevault-excluded`; a ticket or
terminal route leaking into `paths` fails the unit suite. The current build
serializes **553 paths / 696 operations** and names 30 excluded transports.

- Unit (`src/openapi.rs`, 10): method chains + nested prefix parsing, commented
  routes ignored, the real router/sub-routers present, OpenAPI 3 and contract
  version, public override (no `401` advertised), CSRF on mutations only,
  every `{param}` declared `required`, sensitive transports excluded and
  listed, deterministic serialization.
- Integration (`tests/openapi_integration.rs`, 2): anonymous `401`, member
  `403`, admin `200`; the document shape, nested `/api/network/interfaces`,
  public login security override, exclusions, and two reads byte-identical.
- Docs: `docs/API.md` → "OpenAPI 3"; `docs/ARCHITECTURE.md`,
  `docs/BACKEND.md`, `docs/INDEX.md`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run` and
`cargo check --test openapi_integration` are clean (pre-existing warnings
only). Run before release: `cargo test --lib openapi` and
`cargo test --test openapi_integration`.

---

## 2026-11-05 — OVH / Namecheap / deSEC dynamic DNS providers

Closes tasklist **P1-5**.

**Behaviour.** `DdnsProvider` gained `ovh`, `namecheap` and `desec`. OVH
DynHost sends HTTP Basic auth (`username` + the DynHost password kept in the
write-only `token`); Namecheap needs `domain` because its API takes host and
registered domain separately, derives the host (`@` for the apex) and accepts
only `<ErrCount>0</ErrCount>`; deSEC sends `Authorization: Token …` to
`update.dedyn.io/nic/update`. `HttpRequest` gained a generic `authorization`
header and `basic_auth` credentials, so no new secret is placed in a URL.

**Tests.**

- Unit (`src/ddns.rs`, 24 — +7): OVH Basic auth with the password absent from
  the URL, OVH username requirement, Namecheap host/domain split and apex `@`,
  Namecheap domain-mismatch refusal, deSEC `Token` header, `good`/`nochg`
  acceptance, and Namecheap XML refusal on `200`.
- Integration (`tests/ddns_integration.rs`, 8 — +1): OVH username, Namecheap
  registered domain and deSEC token round-trips through the admin API, each
  refusal persisting nothing; defaults now expose `username`, `domain` and the
  seven-provider list.
- E2E (`tests/e2e/settings/ddns.spec.ts`, 8 — +1): 1.1 expects the seven
  provider values; 1.2 checks the OVH username / Namecheap domain fields
  appear only for their provider.

**Docs.** `docs/guides/ddns.md` (fields + seven-provider request table),
`docs/API.md` (payload), `docs/INDEX.md`, the `ddns` RAG `FeatureDoc`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run`,
`cargo test --test ddns_integration --no-run` and `npm run check` are clean
(pre-existing warnings only). Run before release:
`cargo test --lib ddns`, `cargo test --test ddns_integration`, then
`npx playwright test tests/e2e/settings/ddns.spec.ts`.

---

## 2026-11-05 — Incus clone, and the VM capability status

Closes tasklist **P1-6**.

**Behaviour.** `POST /api/vm/{name}/clone` duplicates a machine under a new
name, optionally from one of its saved states. The request is `{ name,
snapshot?, profiles? }`; Incus receives `source.type = "copy"`,
`source = /1.0/instances/<source>` and `source_snapshot`, copies the source's
config/devices and keeps the clone **stopped**. The handler validates the new
name, the source snapshot name, the name collision and the source existence
before calling Incus, waits up to `CLONE_WAIT_SECS = 300`, and answers
`status:"cloned"`, `status:"cloning"` + the operation id on an expired wait, or
the provider's own failure. The drawer gains a Clone action with a new-name
field and an optional "copy from" saved-state picker.

**Capability contract fix.** `incus_err(NotAvailable)` now answers **`503`**
via `ResponseHelper::json_unavailable` instead of `400`, so a missing Incus is
a named capability failure across every VM endpoint (the integration suite and
its header were updated). Validation failures remain `400`.

**Tests.**

- Unit (`src/incus.rs`, 23 — +1): `InstanceSource::copy` serializes the copy
  type, encoded source path and optional `source_snapshot`.
- Unit (`src/handlers/vm.rs`, 30 — +1): `clone_body` copies the source type and
  profiles, stays stopped, omits config/devices so Incus keeps the source's,
  honours a profile override and falls back to `default`.
- Integration (`tests/vm_integration.rs`, 30 — +2): clone joins the auth/route
  matrix; a bad name, same name and malformed snapshot are refused before
  Incus; a valid clone names the missing Incus with `503`; existing
  unavailable-Incus assertions moved from `400` to `503`.
- E2E (`tests/e2e/vm/vm.spec.ts`, 46 `test(` — +2): 4.7 rejects a bad clone
  name before Incus; 4.8 asserts the `503` + `incus` sentence when the daemon
  is absent and skips when it is present (never clones a real machine).

**Docs.** `docs/guides/vm.md` (endpoint, request shape, long operations),
`docs/API.md`, `SCREENS.md`, the `virtual-machines` RAG `FeatureDoc`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run`,
`cargo test --test vm_integration --no-run` and `npm run check` are clean
(pre-existing warnings only). Run before release:
`cargo test --lib vm`, `cargo test --test vm_integration`, then
`npx playwright test tests/e2e/vm/vm.spec.ts -g "4.7|4.8"` (and the rest of
the suite).

---

## 2026-11-05 — static IPv4/IPv6 routes

Closes tasklist **P1-7**.

**Behaviour.** `src/static_routes.rs` manages routes for networks reached
through another gateway. `GET /api/network/routes` (auth) reports the saved
list with an `active` flag and `available` (is `ip` installed);
`POST`/`DELETE` are admin-only. A route is validated (CIDR with non-zero
prefix, same-family unicast next hop, interface ≤ 15 chars, metric 0–65535),
applied with argv `sudo ip -4|-6 route replace …`, read back from the kernel,
then saved to `data/static_routes.json`. A save failure restores the previous
route (or removes the new one when none existed); one route per destination.
At boot a task waits 20 s, re-applies the saved list and keeps entries whose
interface was not up yet. Missing `ip` answers `503` named; malformed requests
stay `400`. The Network screen gains a **Routes** card.

**Tests.**

- Unit (`src/static_routes.rs`, 6): destination canonicalisation and refusals
  (including `0.0.0.0/0` / `::/0`), mismatched/unusable next hop, unsafe
  interface names, metric bounds, exact IPv4/IPv6 argv (never a shell).
- Integration (`tests/network_integration.rs`, 29 — +1): anonymous `401`,
  member read `200`, member mutation `403`, admin refusals for a bad CIDR,
  default route, cross-family via, unknown interface and out-of-range metric
  before any `ip route`, and an unknown delete.
- E2E (`tests/e2e/network/network.spec.ts`, 49 `test(` — +2): the Routes card
  and its labelled controls; an impossible destination is refused and the
  stored route count is unchanged. No real route is ever added.
- Docs: `docs/guides/network.md` → "Static routes", `docs/API.md`,
  `SCREENS.md`, the `network` RAG `FeatureDoc`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run`,
`cargo test --test network_integration --no-run` and `npm run check` are clean
(pre-existing warnings only). Run before release:
`cargo test --lib static_routes`, `cargo test --test network_integration`,
then `npx playwright test tests/e2e/network/network.spec.ts -g "15"`.

## Shares hardening — P0/P1/P2, 2026-11-08 (tests added, not executed)

Per the implementation instruction for this pass, **no test was executed**.
The suite compiles (`cargo check --tests`), but the counts above are the last
executed baseline and must be refreshed by running:

```bash
cargo test --lib
cargo test --test shares_integration
cd frontend && npm run check
npx playwright test tests/e2e/shares/shares.spec.ts
```

New coverage added in this pass:

| Area | Test |
|---|---|
| Samba credential plumbing | `src/host_accounts.rs` unit tests: `smbpasswd_input`, injection refusal, idempotent delete/lock |
| SMB share access generation | `src/shares.rs` unit tests: `smb_valid_users`, `SmbShareAccess::render`, managed-line rewrite |
| NFS per-client rules | `src/shares.rs` unit tests: per-client RO/RW rendering, duplicate/injection refusal |
| NFS Kerberos | `src/shares.rs` unit test: `krb5` refused on v3, accepted and rendered on v4.2 |
| SMB ADS | `src/shares.rs` unit test: realm/idmap/ACL mapping rendering |
| Previous Versions | `src/shares.rs` unit test: `shadow_copy2` block round-trip |
| FTP | `src/ftp.rs` unit tests: strictest-permission merge, user config, invalid-name purge; `src/shares.rs` `anon_root` rendering |
| WebDAV grants | `src/webdav/vfs.rs`: `deny` wins, `read` cannot write |
| Recursive ACL walk | `src/acl.rs`: walk visits every directory/file, bounded by `MAX_RECURSIVE_ENTRIES` |
| API contract | `tests/shares_integration.rs`: logical definitions, previous-versions read, NFS security flavor, domain join refused before ADS config, unknown logical share, matrix |
| E2E contract | `tests/e2e/shares/shares.spec.ts`: protocol checkbox group replaces the old select; SMB security select includes `ads` |
---

## 2026-11-05 — integrity lot 1 (P0-5, P0-9, P0-11)

Closes tasklist **P0-5**, **P0-9** and **P0-11**.

**Behaviour.**

- **P0-5 — USB Copy.** `verify_copy` now reads every copied file and compares
  a SHA-256 of source and destination, collects every mismatch, and fails the
  copy with the list. A single-file source is copied under its own name and
  counted as one file. The old "same size = verified" path is gone.
- **P0-9 — configuration writes.** `AtomicWriter` no longer removes the target
  before `rename`: it writes a unique temporary file in the same directory,
  fsyncs it, renames it over the target, fsyncs the parent directory, and
  preserves the existing mode. `write_config_file` keeps a `.bak`;
  `append_config_file` becomes a locked read-modify-write. `apply_smb_config`
  validates the candidate with `testparm -s` before replacing `smb.conf`, and
  `self_heal_smb_config` restores an empty/invalid file from `.bak` or the
  defaults at service start. Sudoers grants cover the `.bak` paths.
- **P0-11 — SMB.** `SmbConfig` gains `durable_handles`, `smb_encrypt`
  (`off|desired|required`), `server_signing` (`off|desired|mandatory`),
  `strict_sync`, `aio_read_size`, `aio_write_size`, `use_sendfile`, `oplocks`,
  `kernel_oplocks` and `level2_oplocks`, all rendered exhaustively, validated,
  normalized and exposed in the advanced SMB form (i18n ×4).

**Tests.**

- Unit (`src/storage/atomic.rs`, 6): replacement without a temporary, missing
  file creation, mode preservation, binary round-trip, read/missing, unique
  temporary names.
- Unit (`src/shares.rs`, +6): atomic replace + `.bak`, append preservation,
  self-heal from `.bak`, valid file untouched, new-field rendering/contract,
  deserialization defaults, invalid value normalization.
- Unit (`src/usb_copy.rs`, +6): identical tree, same-size different bytes,
  size mismatch, missing file, single-file source, single-file count.
- Integration (`tests/usb_copy_integration.rs`, +1): a corrupted destination
  is refused with a checksum error. `tests/shares_integration.rs` still
  compiles against the new `SmbConfig`.
- Docs: `docs/guides/shares.md`, `docs/guides/usb-copy.md`, `SCREENS.md`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run`,
`cargo test --test usb_copy_integration --no-run` and
`cargo test --test shares_integration --no-run` compile clean (pre-existing
warnings only). `npm run check` could not run in this worktree
(`svelte-check` not installed). Run before release:
`cargo test --lib storage::atomic shares::tests usb_copy`,
`cargo test --test usb_copy_integration`,
`cargo test --test shares_integration`, then `npm --prefix frontend ci &&
npm --prefix frontend run check` and the Shares/USB Copy E2E.

---

## 2026-11-05 — integrity lot 2 (P0-7, P0-6 partial)

Closes tasklist **P0-7** and the scheduled/automatic half of **P0-6**.

**Behaviour.**

- **P0-7 — boot recovery.** `src/storage/recovery.rs` runs before the share
  services: `mdadm --assemble --scan`, `btrfs device scan`, then every
  HomeVault-marked `/etc/fstab` entry that is not in `/proc/mounts` is retried
  with its exact fstab options. A failed Btrfs mount goes through
  `btrfs rescue super-recover -y` then a read-only mount; ext4 goes through
  `dumpe2fs -h` (not-clean detection), `e2fsck -p` (preen, never `-y`), the
  normal mount, `ro,noload`, then `ro`. The report is written to
  `data/state/storage-recovery.json` and logged.
- **Clean-shutdown marker.** `shutdown_signal` now handles **SIGTERM** as well
  as Ctrl+C and writes `data/state/clean-shutdown` after the existing cleanup.
  The next boot treats a missing marker (after the first boot) as an unclean
  shutdown and schedules `scrub_after_unclean_shutdown`: two-minute delay,
  then one scrub per mounted, non-degraded pool, sequential through
  `src/scrub.rs`.
- **P0-6 — scheduled scrub.** The generic task engine gains
  `TaskAction::Scrub { pool, pool_type }` (`zfs|btrfs|mdadm`). It calls
  `src/scrub.rs`, which owns the single-flight lock, the 30 s start deadline
  and the mdadm device derivation (`md0` → `/dev/md/md0`). The Tasks screen
  offers the new type with pool + storage-type fields (i18n ×4). True
  resume-after-reboot of a partially completed scrub is **not** implemented;
  the schedule re-runs it, and ZFS resumes on its own.

**Tests.**

- Unit (`src/storage/recovery.rs`, 7): fstab marker parsing, marker with no
  entry, `/proc/mounts` matching + `\040` unescape, ext4 clean/not-clean,
  fallback ladder, first/clean/unclean boot markers, marker reset.
- Unit (`src/scrub.rs`, 3): kind parsing, mdadm device derivation, stable
  wire names.
- Unit (`src/scheduled_tasks_scheduler.rs`, +1): scrub payload validation
  (valid zfs, spaces refused, ext4 refused).
- Integration (`tests/scheduled_tasks_integration.rs`, +2): a scrub task is
  created for ZFS and refused for ext4.
- Docs: `docs/guides/storage.md` → "Boot recovery",
  `docs/guides/scheduled-tasks.md` → `scrub`, `SCREENS.md`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run` and
`cargo test --test scheduled_tasks_integration --no-run` compile clean
(pre-existing warnings only). `npm run check` could not run in this worktree.
Run before release: `cargo test --lib storage::recovery scrub
scheduled_tasks_scheduler`, `cargo test --test scheduled_tasks_integration`,
then the Tasks E2E.

---

## 2026-11-05 — integrity lot 3 (P0-8)

Closes tasklist **P0-8** (core).

**Behaviour.** Migration `089_disk_health.sql` adds `disk_health_samples`
(one row per watched SMART attribute per run) and `disk_health_findings`
(deduplicated by `serial:attr:condition`). `src/disk_health.rs` samples
5/9/177/187/188/190/194/197/198/199/231, applies the thresholds (5 +5,
187/197/198 on increase, wear ≤10 %, temperature ≥55 °C, `-H` FAILED), and
adds system findings from `journalctl` (24 h), `btrfs device stats`,
`zpool status -x` and `mdadm --detail` mismatch. A finding is notified once
through `health.disk_smart`; the repository's `is_active` check prevents a
daily repeat. `src/disk_health_scheduler.rs` samples at most once per 20 h.
Endpoints: `GET /api/health/disk-health` (session),
`GET /api/health/disk-health/samples` (session),
`POST /api/health/disk-health/sample` (admin). The Health screen gains a
**Disk health history** card with the findings and **Sample now**.

**Tests.**

- Unit (`src/disk_health.rs`, 8): SMART row parsing (two-token names,
  multi-token RAW_VALUE), PASSED/FAILED, reallocated +5, pending-sector
  first/increase, wear/temperature, journal I/O filtering, btrfs counters,
  zpool health + mdadm mismatch.
- Unit (`src/disk_health_scheduler.rs`, 2): missing/malformed timestamp is
  due; 19 h is reused and 21 h is due.
- Integration (`tests/health_integration.rs`, +3): findings require a session
  and start empty; samples validate `serial` and return an empty history;
  sample-now is refused to a member and reaches the sampler for an admin.
- Docs: `docs/guides/health.md` → "Disk health history".

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run` and
`cargo test --test health_integration --no-run` compile clean (pre-existing
warnings only). `npm run check` could not run in this worktree. Run before
release: `cargo test --lib disk_health`, `cargo test --test
health_integration`, then the Health E2E.

---

## 2026-11-05 — integrity lot 4 (P0-10)

Closes tasklist **P0-10** via the generic scheduled-task engine.

**Behaviour.** `ResticService::check_repo_with_subset` runs `restic check
--read-data-subset=<subset>`. `TaskAction::ResticCheck { repo_id, subset }`
validates the id and the subset (`10%`, `1/4`, `1G`; empty = `10%`), loads the
repository by id, and runs the check on the blocking pool. The task history
(status/output/date) is the last-check record; the Tasks screen gains the
**Restic repository check** type with repository-id and subset fields
(i18n ×4). A `--force`-style subset is refused at creation and again at run
time.

**Tests.**

- Unit (`src/scheduled_tasks_scheduler.rs`, +1): valid repo/subset, empty
  subset defaults to `10%`, `../etc` repo refused, `--force` refused,
  `10%`/`1/4`/`1G` accepted, `0%` and `2/1` refused.
- Integration (`tests/scheduled_tasks_integration.rs`, +2): a `restic_check`
  task is created; an option-injection subset is refused.
- Docs: `docs/guides/scheduled-tasks.md` → `restic_check`, `SCREENS.md`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run` and
`cargo test --test scheduled_tasks_integration --no-run` compile clean
(pre-existing warnings only). `npm run check` could not run in this worktree.
Run before release: `cargo test --lib scheduled_tasks_scheduler`,
`cargo test --test scheduled_tasks_integration`, then the Tasks E2E.

---

## 2026-11-05 — integrity lot 5 (P0-12, complete)

Closes tasklist **P0-12** (write semantics + full-volume protection).

**Behaviour.** Migration `090_iscsi_write_semantics.sql` adds
`write_policy` (`buffered|direct`), `emulate_fua` and `unmap` to
`iscsi_backstores`. `src/iscsi.rs` emits three `targetcli … set attribute`
commands after the backstore is created (`emulate_write_cache`,
`emulate_fua_write`, `emulate_tpu`); a failure stops the apply. The handler
validates the policy, the repository persists it, and the iSCSI form exposes
**I/O policy**, **Honour FUA** and **Advertise UNMAP** (i18n ×4).
`docs/guides/iscsi.md` and `SCREENS.md` document the buffered/direct trade-off.

**Full-volume guard.** `src/iscsi.rs` exposes `free_bytes_at` and
`should_protect_from_full_volume` (fileio only, writable only, free space
under `VOLUME_FULL_RESERVE_BYTES`). `src/iscsi_guard.rs` runs a pass at
startup (3 min delay) then every 5 min: it reads each backstore, and when one
is nearly full — or its free space cannot be measured — it sets
`readonly = true` through `IscsiRepository::set_backstore_readonly` and raises
a notification. Marking read-only is reversible; a corrupt backing file is
not. `run_apply` is now `pub(crate)` so the guard can rebuild the target.

**Tests.**

- Unit (`src/iscsi.rs`, +2): buffered → `emulate_write_cache=1`/FUA/UNMAP on,
  direct → all three `=0`; `valid_write_policy` accepts only the two values.
- Unit (`src/iscsi.rs`, +2): a nearly-full or unreadable volume protects only
  writable fileio stores; a block or already-read-only store is skipped;
  `free_bytes_at` reads a real filesystem and rejects a bad path.
- Unit (`src/iscsi_guard.rs`, 2): low/unreadable volume → protect; block or
  already read-only → skip.
- Integration (`tests/iscsi_integration.rs`, +2): a direct/FUA-off/UNMAP-off
  backstore round-trips through the API; `write_policy=writeback` is refused.
- Docs: `docs/guides/iscsi.md`, `SCREENS.md`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo check --lib`, `cargo test --lib --no-run` and
`cargo test --test iscsi_integration --no-run` compile clean (pre-existing
warnings only). `npm run check` could not run in this worktree. Run before
release: `cargo test --lib iscsi`, `cargo test --test iscsi_integration`,
then the iSCSI E2E and a real configfs apply.

---

## 2026-11-05 — integrity lot 6 (P0-6 complete, P1-21…P1-28, P2-10…P2-15)

Closes the exact-resume half of **P0-6** and the P1/P2 reliability lot. Three
items ship as reviewed infrastructure only and are called out below.

**P0-6 — exact scrub resume.** `src/scrub.rs` writes
`state/scrub-in-progress` (`pool=…`, `kind=…`) only after the start command
succeeded. `resume_decision` classifies the live status: a scrub still
`in progress`/`running` is left alone, `finished`/`repaired`/`no errors`
clears the marker, and `idle`/an unknown output means the recorded scrub was
interrupted → `resume_interrupted` starts it again from the beginning.
`src/storage/recovery.rs` calls it at boot **before** the post-unclean-shutdown
global scrub. ZFS may resume its own scan; Btrfs/mdadm restart.
- Unit (`src/scrub.rs`, +2): marker round-trip/unknown-field tolerance, and
  finished/running never restarted.

**P1-21 — RAID stability.** `src/raid_stability.rs`: `SpeedProfile`
(low/balanced/fast) with ordered limits, `parse_monitor_event` for the
`md/array <event> <device>` line, `pick_spare`, `degraded_transitions`, and a
poll loop (2 min delay, 5 min interval) spawned by the web process. Nothing
calls `apply_speed_profile` yet, and adding a picked spare is not automatic.
- Unit (4): closed speed set/limits, event parsing, spare exclusion,
  clean→degraded-only notification.

**P1-23 — foreign mdadm import.** `list_importable_mdadm_pools` parses
`mdadm --examine --scan`, `assembled_md_names` drops already-assembled arrays,
and import runs `mdadm --assemble --scan` then verifies with `--detail`.
- Unit in `src/pools.rs` covers the scan parser and the assembled-name filter;
  the import test was adapted to the new helper.

**P1-22 / P2-11 — Btrfs maintenance.** `src/btrfs_maintenance.rs` supplies
the live options (`compress=zstd:3,space_cache=v2,noatime`), usage parsing,
`balance_needed` (>4 % chunk use **and** <5 GiB unallocated), balance/trim/
stats argv, `validate_compression_level` (`zstd:1..15`), and
`defrag_command` refusing `/.snapshots`. `src/pools.rs` applies the options
for new pools and boot-recovery mounts. No balance/trim/defrag scheduler is
wired; the commands are available to the admin/task engine.
- Unit (5): options with/without subvolume, size/percentage parsing,
  conditional balance, argv shape, compression + snapshot defrag refusal.

**P1-24 — transfer digests.** Explorer downloads hash files ≤64 MiB
automatically and larger files only with `?digest=true`; the SHA-256 is sent
in `Repr-Digest`/`Content-Digest`. Chunk uploads validate `X-Chunk-Sha256`
before commit and roll back a mismatching chunk with `422`.
- Unit (`src/handlers/explorer.rs`): the chunk cap, over-cap rollback, and a
  corrupt chunk rejected before commit. Integration
  (`tests/explorer_integration.rs`): a download carries the real digest
  header.

**P1-25 — final sync verification.** rclone runs `--checksum`,
`--retries 3`, `--low-level-retries 10`, `--timeout 5m`, `--contimeout 1m`
and, after success, `rclone check --one-way --checksum`; a difference fails
the run. The cache mover runs `rsync -a --checksum --dry-run
--itemize-changes` after a copy (skipped with `--remove-source-files`).
- `rclone_check_args` is a pure argv builder; it is covered by compilation
  only (no unit test asserts its shape yet). The cache path compiles against
  the new verification block.

**P1-26 — Btrfs over SSH + destination proof.** `validate_job` no longer
refuses Btrfs over SSH; the receive/list/delete commands use the shared
`ssh_prefix`. A failed transfer is retried three times (2/4/8 s). After
success `destination_verify_command` reads the ZFS `checksum` property or runs
`btrfs subvolume show` (locally or over SSH); failure fails the run.
- Unit (`src/replication.rs`): all four pipeline shapes (local/SSH ×
  ZFS/Btrfs) and the SSH destination list/delete argv.
- Integration (`tests/replication_integration.rs`): the old "Btrfs over SSH
  is refused" assertion was removed; the remaining contract still compiles.

**P1-27 — supply chain.** `src/supply_chain.rs` provides strict SHA-256
syntax, `pinned_digest`/`is_pinned`, `verify_ed25519`/`verify_artifact`.
`DockerService::pull_image` calls `validate_image_reference` before `docker
pull`.
- Unit (2): digest syntax/pinning, malformed `@sha256:` refusal.
- **Reserve:** catalog manifests are not required to be digest-pinned or
  signed, and no Ed25519 key is configured in this build.

**P1-28 — system stability.** `installer/install.sh` writes
`LimitNOFILE=65536`, `TimeoutStopSec=30`, start limits and `OOMPolicy=stop`,
plus `/etc/sysctl.d/99-homevault.conf` (`kernel.panic=10`,
`panic_on_oops=1`, dirty ratios 5/10). `src/system_stability.rs` holds the
same values and the pure `parse_write_cache`, `write_cache_warning` and
`duplicate_monitor_warning` helpers.
- Unit (4): unit/sysctl policy, hdparm both ways, unprotected cache warning,
  duplicate-monitor warning.
- **Reserve:** the two advisory warnings are not surfaced in the UI yet.

**P2-10 — SMART prediction.** `src/disk_prediction.rs` fits least squares on
attributes 5/187/188/194/197/198 (≥5 samples), reports slope/r²/days-to-
threshold with a residual interval, and `GET
/api/health/disk-health/prediction?serial=…` exposes it.
- Unit (5): no trend below 5 points, stable attribute, rising estimate, noisy
  fit rejected, samples→days conversion.

**P2-12 — weekly health report.** `src/health_report.rs` renders findings,
last sample, pool free space, pending scrub and last restic check, sends at
most weekly through `health.storage`, and maps every missing value to
`unknown`.
- Unit (3): unknown never `OK`, measured rendering, seven-day due rule.
- **Reserve:** the restic-check slot is always `unknown` (run history not
  queried yet).

**P2-13 / P2-15 — policy modules only.** `src/snapshot_quiescence.rs`
(crash vs application consistency, hook plan, downgrade on failure) and
`src/replication_witness.rs` (lease, promotion, split-brain) are unit-tested
but **not wired** to the snapshot scheduler or a witness transport.
- Unit (5 + 3) respectively.

**P2-14 — SQLite durability.** `HOMEVAULT_SQLITE_FULL_SYNC=1|true|yes|full`
switches `PRAGMA synchronous` to `FULL`; the default stays `NORMAL`.
- Unit (`src/storage/database.rs`): the accepted spellings are the only ones
  that select `FULL`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. Verified: `cargo check --lib`, `cargo test --lib --no-run`,
and `cargo test --test <suite> --no-run` for `replication_integration`,
`iscsi_integration`, `storage_integration`, `raid_integration`,
`rclone_integration`, `health_integration`, `cache_integration`,
`explorer_integration`, `setup_integration` and
`scheduled_tasks_integration` — all compile with pre-existing warnings only.
`npm run check` could not run in this worktree (`svelte-check` absent).
Run before release: the unit filters above, the ten integration suites, then
the Tasks/Health/Storage/Replication/iSCSI/Cloud Sync E2E and a real-host pass
for the sysctl/unit and mdadm paths.

---

## 2026-11-05 — integrity lot 7 (reserves: P0-7/8/10, P1-21/22/25/27/28, P2-12)

Closes the follow-ups recorded in lot 6 where the work was small enough to
land without a design change.

**P0-7 — fstab timeouts.** New `FSTAB_SYSTEMD_OPTIONS`
(`x-systemd.device-timeout=30s,x-systemd.mount-timeout=30s`) is written by the
Btrfs, RAID and Btrfs-layout fstab writers. `manual_mount_options` strips
`x-systemd.*` before the boot-recovery retry replays the entry with `mount(8)`
— those options are systemd-only, and passing them to `mount` would fail for
the wrong reason.
- Unit (`src/btrfs_maintenance.rs`): fstab options keep the mount options and
  add both timeouts.
- Unit (`src/storage/recovery.rs`): the stripper keeps real options and falls
  back to `defaults`.

**P0-8 — temperature streak.** A single hot sample no longer alerts.
`analyze_temperature` requires the current sample plus the two stored ones to
all be ≥ 55 °C; `recent_raws` loads them. Airflow (190) and temperature (194)
keep distinct fingerprints.
- Unit (`src/disk_health.rs`): one/two hot samples are ignored, three alert,
  a cool point breaks the streak, 190 vs 194 fingerprints.

**P0-10 — restic retry + notification.** A failed `restic check` is retried
three times with 5 s/30 s backoff; repository findings are not retried. A
failed run also raises `backup.jobs` with the task name and error.
- Unit (`src/scheduled_tasks_scheduler.rs`): backoff values and the
  restic-only notification predicate.

**P1-21 — spare suggestion.** The degradation notification now names a free,
non-failed candidate (`spare_suggestion`) or says explicitly that no spare is
available. `mdadm --add` remains a human action.
- Unit (`src/raid_stability.rs`): candidate sentence and no-spare sentence.

**P1-22 — Btrfs maintenance loop.** A 24 h loop (1 h startup delay) checks
`btrfs filesystem usage` and runs the conservative balance only when
`balance_needed`, plus a weekly `fstrim` tracked in `state/btrfs-last-trim`.
Read-only mounts and `.snapshots` are skipped; `HOMEVAULT_BTRFS_MAINTENANCE=0`
disables both.
- Unit (`src/btrfs_maintenance.rs`): opt-out parsing, weekly due rule,
  mount filtering.

**P1-25 — transfer history.** Every rclone sync (button and scheduled job)
writes a `transfer_history` row (`operation=copy`, `method=rclone-check`) with
`verified=true` only after a successful non-dry-run check; failures carry the
message.
- Unit (`src/handlers/rclone.rs`): `sync_verified` and `rclone_check_args`.

**P1-27 — pinned images.** `HOMEVAULT_REQUIRE_PINNED_IMAGES=1` makes a catalog
install refuse any rendered compose image that is not `@sha256:…`, listing the
offenders before `docker compose` runs. Off by default.
- Unit (`src/supply_chain.rs`): compose parsing, strict-mode refusal, digest
  acceptance.

**P1-28 — duplicate monitors.** The installer warns when `smartd` or
`mdmonitor` is enabled beside HomeVault's own samplers. The UI surface for the
write-cache/UPS advisory is still open.

**P2-12 — restic check in the weekly report.** The report reads the newest
`restic_check` task run (`status (finished_at)`) instead of always rendering
`unknown`; `HOMEVAULT_WEEKLY_HEALTH_REPORT=0` disables the report.
- Unit (`src/health_report.rs`): environment opt-out.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt`, `cargo check --lib` and
`cargo test --lib --no-run` compile clean (pre-existing warnings only). Run
before release: `cargo test --lib` for the filters above, the ten integration
suites from lot 6, and the Storage/Health/Tasks/Cloud Sync E2E.

---

## 2026-11-05 — integrity lot 8 (P2-10, P2-11)

**P2-10 — prediction UI.** `GET /api/health/disk-health/predictions` returns
`[{ serial, trends[] }]` for every sampled disk (`list_serials`), without
inventing a trend below 5 samples. The Health screen gains a **Failure
prediction** card showing attribute, estimated days and `r²`; it stays hidden
until at least one trend exists.
- Integration (`tests/health_integration.rs`): the new route requires a
  session, starts empty and the per-serial route still validates `serial`.
- i18n ×4 for the card and the six attribute names.

**P2-11 — per-share compression policies.** Migration
`091_btrfs_compression.sql` adds `btrfs_compression_policies` (path, level,
defrag, last_applied_at). `src/storage/btrfs_policy_repo.rs` owns CRUD;
`src/btrfs_maintenance.rs` validates paths (`absolute`, no `..`, never
`.snapshots`), builds the defrag/`chattr` argv, and applies a policy now or
when the daily loop finds it due (monthly). Endpoints:
`GET/PUT/DELETE /api/storage/btrfs/policies` and
`POST /api/storage/btrfs/policies/apply`.
- Unit (`src/btrfs_maintenance.rs`): path validation, `zstd:3`/`none` argv,
  `+c`/`-c`, monthly due rule.
- Integration (`tests/storage_integration.rs`): auth, invalid level refusal,
  round-trip and delete of a policy against a real temporary directory.
- UI: Storage → **Compression** tab (list, path/level/defrag form, Apply now,
  Delete); i18n ×4.
- Docs: `docs/guides/storage.md`, `docs/API.md`, `SCREENS.md`.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run`, `cargo test --test storage_integration --no-run`
and `cargo test --test health_integration --no-run` compile clean. Run before
release: `cargo test --lib btrfs_maintenance disk_prediction`,
`cargo test --test storage_integration health_integration`, then the
Health/Storage E2E.

---

## 2026-11-05 — integrity lot 9 (P2-13)

**Behaviour.** Migration `092_snapshot_quiescence.sql` adds
`snapshot_quiescence_hooks` (name, `pre_json`, `post_json`, enabled).
`src/storage/quiescence_hook_repo.rs` owns CRUD. The snapshot scheduler loads
the enabled hooks before every scheduled snapshot, runs their `pre` argv in
name order inside `snapshot_quiescence::run_pre_hooks`, creates the snapshot,
then runs the attempted hooks' `post` argv in reverse order. A failed
pre-hook downgrades the run to `crash-consistent: <reason>` without aborting
it; a failed post-hook is appended to the summary. Every command is an argv
(no shell), absolute, ≤32 arguments, 60 s deadline.

**Endpoints/UI.** `GET/POST /api/storage/snapshot-hooks`,
`PUT/DELETE /api/storage/snapshot-hooks/{id}`; Storage → Snapshots gains an
**Application quiescence hooks** card (add, enable/disable, delete), i18n ×4.

**Tests.**
- Unit (`src/snapshot_quiescence.rs`): absolute argv, required pre, 32-arg
  cap, name validation.
- Unit (`src/snapshot_scheduler.rs`): `consistency_note` says
  crash/application-consistent and reports post-hook failures.
- Integration (`tests/storage_integration.rs`): auth, relative executable
  refused, create/list/update/delete round-trip.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run` and `cargo test --test storage_integration
--no-run` compile clean. Run before release: `cargo test --lib
snapshot_quiescence snapshot_scheduler`,
`cargo test --test storage_integration`, then the Storage snapshots E2E.

---

## 2026-11-05 — integrity lot 10 (P2-15)

**Behaviour.** `src/replication_witness.rs` gains an HTTP `WitnessClient`
speaking `GET/POST/DELETE /lease` with an optional bearer token and a 10 s
deadline, plus `witness_config_from_env` (`HOMEVAULT_WITNESS_URL`,
`HOMEVAULT_WITNESS_NODE`, `HOMEVAULT_WITNESS_TOKEN`, `HOMEVAULT_WITNESS_TTL`).
Every replication run acquires/renews the lease before touching a snapshot
when a witness is configured; a `409`/wrong-holder answer fails the run with a
named reason. Without configuration nothing changes. `GET/POST
/api/replication/witness` expose the current lease and an explicit acquire
(admin).

**Tests.**
- Unit (`src/replication_witness.rs`): all-or-nothing configuration, URL
  scheme refusal, required node, default/explicit TTL, lease URL; existing
  `may_promote`/`take_lease`/`split_brain` tests.
- Integration (`tests/replication_integration.rs`): witness status requires a
  session, reports `configured:false`, acquire is admin-only and names the
  missing configuration.

**Reserve.** The witness server is an external service; the promotion and
split-brain recovery runbook is not automated, and the lease is renewed only
at run start (its TTL gates the start, not a multi-hour transfer).

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run`, `cargo test --test replication_integration
--no-run` and `cargo test --test storage_integration --no-run` compile clean.
Run before release: `cargo test --lib replication_witness
snapshot_quiescence snapshot_scheduler`,
`cargo test --test replication_integration storage_integration`, then a
two-node + witness integration test on real hosts.

---

## 2026-11-05 — integrity lot 11 (P2-1, partial)

**Behaviour.** `SmbConfig` gains `realm` and `idmap_range`. `security_mode =
ads` (and `domain`) is accepted only with a DNS realm and a `low-high` idmap
range; `server` stays refused. The rendered `[global]` block adds `realm`,
`idmap config * : backend = tdb`, the idmap range,
`winbind use default domain = yes`, `vfs objects = acl_xattr`,
`map acl inherit = yes` and `store dos attributes = yes`. A stored ADS config
without a realm is normalized back to `user`.

`POST /api/shares/smb/domain/join` and `/leave` run `net ads join|leave -U
<account>` with the password written on **stdin**, never in argv; a host
without winbind gets a named refusal. The SMB advanced form offers ADS and
shows the realm/idmap fields.

**Tests.**
- Unit (`src/shares.rs`): ADS requires realm, realm/idmap validation, ads
  directives rendered, `domain_join_argv`/`domain_leave_argv` contain no
  password, legacy normalisation.
- Integration (`tests/shares_integration.rs`): join/leave refuse an
  unauthenticated caller and a member; the admin path is not run because it
  would execute `net ads` against the host.

**Reserve.** The join path is not exercised against a real domain controller;
PAM/local login integration, SID resolution tests and Windows ACL round-trips
still need a test DC. The task stays partial.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run` and `cargo test --test shares_integration
--no-run` compile clean. Run before release: `cargo test --lib shares`,
`cargo test --test shares_integration`, then a lab with Samba + winbind + a
test AD domain.

---

## 2026-11-05 — integrity lot 12 (P2-2, partial)

**Behaviour.** `NfsConfig` gains `security_flavor` (`sys|krb5|krb5i|krb5p`).
A non-`sys` flavor is rendered as `sec=…` on every export and is refused
unless `version` is explicitly `4`, `4.1` or `4.2` (auto cannot guarantee v4).
`save_nfs_config` also refuses Kerberos when `/etc/krb5.keytab` and
`rpc.svcgssd`/`gssproxy` are absent. The NFS advanced form exposes the flavor,
and the hint names the keytab/gssproxy requirement.

**Tests.**
- Unit (`src/shares.rs`): Kerberos requires v4, invalid flavor refused,
  `sec=krb5p` rendered, `sys` omits `sec=`, missing keytab path is not
  considered available.

**Reserve.** Not validated against a real KDC: idmapd configuration, keytab
creation/rotation and client-side `sec=krb5*` mounts still need a lab. The
task stays partial.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run` and `cargo test --test shares_integration
--no-run` compile clean. Run before release: `cargo test --lib shares`,
`cargo test --test shares_integration`, then an NFSv4 + KDC lab.

---

## 2026-11-05 — integrity lot 13 (P2-3, partial)

**Behaviour.** `src/zfs_volumes.rs` creates/list/destroys ZFS zvols. A **thin**
volume adds `-s` and `refreservation=none`; a **thick** one keeps ZFS's default
reservation. Listing parses `zfs list -H -p -t volume` and flags `saturated` at
≥90 % of the nominal size. Names are dataset paths with safe components, and
sizes are 1 MiB–1 PiB; both are validated before any command runs. Endpoints:
`GET/POST /api/storage/zfs/volumes`, `DELETE /api/storage/zfs/volumes/{*name}`.

**Tests.**
- Unit (`src/zfs_volumes.rs`): name/size validation, thin vs thick argv,
  parsing and saturation thresholds.
- Integration (`tests/storage_integration.rs`): auth and validation refusals
  happen before `zfs` is invoked.

**Reserve.** ZFS only: LVM thin pools, a dedicated UI and zvol snapshots are
still open. The task stays partial.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run` and `cargo test --test storage_integration
--no-run` compile clean. Run before release: `cargo test --lib zfs_volumes`,
`cargo test --test storage_integration`, then a real ZFS host.

---

## 2026-11-05 — integrity lot 14 (P2-4, design only)

**Behaviour.** `src/dedup.rs` adds the planning half of deduplication:
strategy selection (`btrfs` → `duperemove`, `zfs` → pool property), path
validation (absolute, no `..`, never `.snapshots`, ≤16 paths), and a
`duperemove` argv builder with an optional persistent hashfile. It **does not
execute** any pass: P2-4 requires the benchmark in
`docs/analysis/dedup-design.md` first.

**Tests.**
- Unit (`src/dedup.rs`): strategy mapping, path rules, path-count cap,
  hashfile validation, argv shape.

**Reserve.** No execution endpoint, no scheduler and no UI. ZFS dedup remains
an operator decision (permanent RAM cost). The task stays partial until the
benchmark is recorded and the pass is enabled.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib` and
`cargo test --lib --no-run` compile clean. Run before release:
`cargo test --lib dedup`, then the benchmark on a real dataset.

---

## 2026-11-05 — integrity lot 15 (P0-7 complete)

**Behaviour.** The boot-recovery pass now raises a `health.storage`
notification when it leaves any entry failed or read-only (previously it only
logged a warning). `storage::recovery::recover_one` is public so the acceptance
test drives the exact production ladder without editing `/etc/fstab`.

**Hardware test.** `tests/recovery_hardware_integration.rs` is opt-in:
`sudo HV_RECOVERY_HW=1 cargo test --test recovery_hardware_integration`.
It creates a 512 MiB loop-backed Btrfs filesystem, zeroes the primary
superblock at 64 KiB, calls `recover_one`, asserts the status is
`mounted-ro`, and verifies the live mount carries `ro`. A skip always prints
the missing prerequisite; loop device and mount point are cleaned up by Drop.

**Not run in this session** (explicit instruction): no `cargo test`, no
Playwright run. `cargo fmt --check`, `cargo check --lib`,
`cargo test --lib --no-run` and `cargo test --test
recovery_hardware_integration --no-run` compile clean. Run before release:
`cargo test --lib storage::recovery`, then the opt-in hardware suite on a
disposable root host.
